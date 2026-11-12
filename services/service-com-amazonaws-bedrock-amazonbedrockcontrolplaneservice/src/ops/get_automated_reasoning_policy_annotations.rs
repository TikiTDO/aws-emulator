// SPDX-License-Identifier: CC0-1.0

//! @op GetAutomatedReasoningPolicyAnnotations
//!
//! Reads back the ANNOTATION SET of a stored Automated Reasoning policy build workflow out
//! of [`crate::state`]: the corrections to the policy's rules, variables and types the
//! workflow carries, the hash that identifies that set
//! ([`crate::state::annotation_set_hash`]) and the instant it was last written — beside the
//! policy the workflow builds and the name that policy was created with.
//!
//! A REPORTING operation: it writes nothing of its own. The one write it makes through the
//! store is the status transition the build workflow's own clock had already made due,
//! which every read of that workflow materialises
//! ([`crate::state::AutomatedReasoningPolicyBuildWorkflowRecord::settle`]), so two reads of
//! one workflow report one annotation set.
//!
//! The set is addressed by the PAIR: an ARN naming no policy of this account, and an id no
//! start minted under that policy, are both refused with `ResourceNotFoundException`, each
//! in the sentence this service answers that miss with.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetAutomatedReasoningPolicyAnnotations`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Retrieves the current annotations for an Automated Reasoning policy build workflow.
//! Annotations contain corrections to the rules, variables and types to be applied to the
//! policy.
//!
//! ## Input
//!
//! * `buildWorkflowId` (AutomatedReasoningPolicyBuildWorkflowId, len ..36, pattern
//!   ^[a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}$) **required**
//!   — The unique identifier of the build workflow whose annotations you want to retrieve.
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the Automated Reasoning policy whose
//!   annotations you want to retrieve.
//!
//! ## Output
//!
//! * `annotationSetHash` (AutomatedReasoningPolicyHash, len 128..128, pattern
//!   ^[0-9a-z]{128}$) **required** — A hash value representing the current state of the
//!   annotations. This is used for optimistic concurrency control when updating annotations.
//! * `annotations` (AutomatedReasoningPolicyAnnotationList, len 0..10) **required** — The
//!   current set of annotations containing rules, variables, and types extracted from the
//!   source documents. These can be modified before finalizing the policy.
//! * `buildWorkflowId` (AutomatedReasoningPolicyBuildWorkflowId, len ..36, pattern
//!   ^[a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}$) **required**
//!   — The unique identifier of the build workflow.
//! * `name` (AutomatedReasoningPolicyName, len 1..256, pattern ^[0-9a-zA-Z-_ ]+$)
//!   **required** — The name of the Automated Reasoning policy.
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the Automated Reasoning policy.
//! * `updatedAt` (Timestamp) **required** — The timestamp when the annotations were last
//!   updated.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
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
// `GetAutomatedReasoningPolicyAnnotationsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetAutomatedReasoningPolicyAnnotationsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#GetAutomatedReasoningPolicyAnnotations")]
#[derive(Debug)]
pub enum GetAutomatedReasoningPolicyAnnotationsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_automated_reasoning_policy_annotations::GetAutomatedReasoningPolicyAnnotationsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetAutomatedReasoningPolicyAnnotationsError = crate::scaffolded::Scaffolded<GetAutomatedReasoningPolicyAnnotationsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetAutomatedReasoningPolicyAnnotationsErrorModeled> for GetAutomatedReasoningPolicyAnnotationsError {
    fn from(modeled: GetAutomatedReasoningPolicyAnnotationsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetAutomatedReasoningPolicyAnnotations` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetAutomatedReasoningPolicyAnnotations<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetAutomatedReasoningPolicyAnnotations<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetAutomatedReasoningPolicyAnnotations<Eng>
{
    type Input = GetAutomatedReasoningPolicyAnnotationsInput;
    type Output = GetAutomatedReasoningPolicyAnnotationsOutput;
    type Error = GetAutomatedReasoningPolicyAnnotationsError;

    const NAME: &'static str = "GetAutomatedReasoningPolicyAnnotations";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_AUTOMATED_REASONING_POLICY_ANNOTATIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_AUTOMATED_REASONING_POLICY_ANNOTATIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                GetAutomatedReasoningPolicyAnnotationsErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some(
                            "automated reasoning policy state is not available on this engine"
                                .to_string(),
                        ),
                    },
                )
                .into(),
            );
        };
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();

        // The policy ARN is matched as the caller spelled it, exactly as a read of the
        // policy itself matches it; the workflow is then looked up under that policy, so an
        // id minted under another policy names no workflow here. The policy is also where
        // the `name` this response carries comes from — the annotations belong to a
        // workflow OF a policy, so one read reports both.
        let Some(name) = store
            .automated_reasoning_policy(&input.policy_arn)
            .map(|policy| policy.name.clone())
        else {
            return Err(not_found(
                crate::state::AUTOMATED_REASONING_POLICY_NOT_FOUND,
            ));
        };
        let Some(workflow) = store.automated_reasoning_policy_build_workflow(
            &input.policy_arn,
            &input.build_workflow_id,
            now,
        ) else {
            return Err(not_found(crate::state::build_workflow_not_found_message()));
        };

        Ok(GetAutomatedReasoningPolicyAnnotationsOutput {
            policy_arn: workflow.policy_arn.clone(),
            name,
            build_workflow_id: workflow.id.clone(),
            annotations: workflow.annotations.clone(),
            // Derived from the set this read reports rather than stored beside it, so the
            // hash a caller holds for optimistic concurrency is the hash of exactly the
            // annotations it was handed.
            annotation_set_hash: crate::state::annotation_set_hash(&workflow.annotations),
            updated_at: crate::state::date_time(workflow.annotations_updated_at),
        })
    }
}

/// The modelled 404 this operation answers for a policy or a build workflow this account
/// holds none of — the annotation set is addressed by the PAIR, so either half missing is
/// "the resource was not found". The wording is the service's own for each resource
/// ([`crate::state::AUTOMATED_REASONING_POLICY_NOT_FOUND`],
/// [`crate::state::build_workflow_not_found_message`]), so a request addressing one is
/// refused with the same sentence wherever it came from.
fn not_found(message: &str) -> GetAutomatedReasoningPolicyAnnotationsError {
    GetAutomatedReasoningPolicyAnnotationsErrorModeled::ResourceNotFoundException(
        ResourceNotFoundException {
            message: Some(message.to_string()),
        },
    )
    .into()
}
