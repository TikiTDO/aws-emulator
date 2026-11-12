// SPDX-License-Identifier: CC0-1.0

//! @op StartAutomatedReasoningPolicyBuildWorkflow
//!
//! Starts a build workflow for a stored Automated Reasoning policy and PERSISTS it in
//! [`crate::state`]: the minted `buildWorkflowId`, the policy it builds, the workflow type
//! the request named, the name, content type and description of the source document
//! that request carried, and the annotation set its repair content carried. The workflow
//! the caller is told about is read back out of the stored record, so what this answers is
//! exactly what `GetAutomatedReasoningPolicyBuildWorkflow` finds, and the annotations it
//! recorded are what `GetAutomatedReasoningPolicyAnnotations` finds.
//!
//! A workflow starts `SCHEDULED` and the clock owes it the whole progression a build runs
//! through — `PREPROCESSING`, `BUILDING`, `TESTING` and then `COMPLETED`
//! ([`crate::state::AutomatedReasoningPolicyBuildWorkflowClock`]) — so a caller reading
//! straight after the start still sees the status the start accepted, and one polling the
//! workflow while it builds reads each processing status in turn.
//!
//! An ARN naming no policy of this account is refused with `ResourceNotFoundException`, a
//! policy one build workflow is still working on is refused with `ResourceInUseException`
//! (one build at a time per policy), a replay of the same `clientRequestToken` answers
//! with the workflow that token already started (`@idempotencyToken`), and a policy
//! already holding as many workflows as it may is refused with
//! `ServiceQuotaExceededException`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `StartAutomatedReasoningPolicyBuildWorkflow`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Starts a new build workflow for an Automated Reasoning policy. This initiates the
//! process of analyzing source documents and generating policy rules, variables, and types.
//!
//! ## Input
//!
//! * `buildWorkflowType` (AutomatedReasoningPolicyBuildWorkflowType, one of:
//!   GENERATE_FIDELITY_REPORT | GENERATE_POLICY_SCENARIOS | IMPORT_POLICY | INGEST_CONTENT |
//!   ITERATIVELY_REFINE_POLICY | REFINE_POLICY | RESOLVE_POLICY_AMBIGUITIES) **required** —
//!   The type of build workflow to start (e.g., DOCUMENT_INGESTION for processing new
//!   documents, POLICY_REPAIR for fixing existing policies).
//! * `clientRequestToken` (IdempotencyToken, len 1..256, pattern
//!   ^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$, @idempotencyToken) — A unique,
//!   case-sensitive identifier to ensure that the operation completes no more than once. If
//!   this token matches a previous request, Amazon Bedrock ignores the request but doesn't
//!   return an error.
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the Automated Reasoning policy for
//!   which to start the build workflow.
//! * `sourceContent` (AutomatedReasoningPolicyBuildWorkflowSource) **required** — The source
//!   content for the build workflow, such as documents to analyze or repair instructions for
//!   existing policies.
//!
//! ## Output
//!
//! * `buildWorkflowId` (AutomatedReasoningPolicyBuildWorkflowId, len ..36, pattern
//!   ^[a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}$) **required**
//!   — The unique identifier of the newly started build workflow. Use this ID to track the
//!   workflow's progress and retrieve its results.
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the Automated Reasoning policy.
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
// `StartAutomatedReasoningPolicyBuildWorkflowError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `StartAutomatedReasoningPolicyBuildWorkflowError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#StartAutomatedReasoningPolicyBuildWorkflow")]
#[derive(Debug)]
pub enum StartAutomatedReasoningPolicyBuildWorkflowErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::start_automated_reasoning_policy_build_workflow::StartAutomatedReasoningPolicyBuildWorkflowError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type StartAutomatedReasoningPolicyBuildWorkflowError = crate::scaffolded::Scaffolded<StartAutomatedReasoningPolicyBuildWorkflowErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<StartAutomatedReasoningPolicyBuildWorkflowErrorModeled> for StartAutomatedReasoningPolicyBuildWorkflowError {
    fn from(modeled: StartAutomatedReasoningPolicyBuildWorkflowErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StartAutomatedReasoningPolicyBuildWorkflow` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct StartAutomatedReasoningPolicyBuildWorkflow<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StartAutomatedReasoningPolicyBuildWorkflow<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StartAutomatedReasoningPolicyBuildWorkflow<Eng>
{
    type Input = StartAutomatedReasoningPolicyBuildWorkflowInput;
    type Output = StartAutomatedReasoningPolicyBuildWorkflowOutput;
    type Error = StartAutomatedReasoningPolicyBuildWorkflowError;

    const NAME: &'static str = "StartAutomatedReasoningPolicyBuildWorkflow";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_AUTOMATED_REASONING_POLICY_BUILD_WORKFLOW_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_AUTOMATED_REASONING_POLICY_BUILD_WORKFLOW_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(server_error());
        };
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();

        // The ARN is matched as the caller spelled it, exactly as
        // `GetAutomatedReasoningPolicy` matches it, so an ARN of another region and a
        // versioned ARN of a version nothing has minted both name no policy to build.
        if store.automated_reasoning_policy(&input.policy_arn).is_none() {
            return Err(
                StartAutomatedReasoningPolicyBuildWorkflowErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(
                            crate::state::AUTOMATED_REASONING_POLICY_NOT_FOUND.to_string(),
                        ),
                    },
                )
                .into(),
            );
        }

        // Every workflow of this policy is read as of `now`, so a workflow the clock has
        // already completed is terminal for the checks below.
        store.settle_automated_reasoning_policy_build_workflows(&input.policy_arn, now);

        // `@idempotencyToken`: a replay answers with what the first request started.
        if let Some(started) = input
            .client_request_token
            .as_ref()
            .and_then(|token| {
                store
                    .automated_reasoning_policy_build_workflow_tokens
                    .get(&(input.policy_arn.clone(), token.clone()))
            })
            .cloned()
        {
            if let Some(existing) = store.automated_reasoning_policy_build_workflow(
                &input.policy_arn,
                &started,
                now,
            ) {
                return Ok(started_workflow(existing));
            }
        }

        // A policy a build workflow is still working on is in use by it, so a second
        // build of that policy is refused rather than queued.
        if let Some(active) = store
            .automated_reasoning_policy_build_workflows_of(&input.policy_arn)
            .find(|record| record.is_active())
        {
            return Err(
                StartAutomatedReasoningPolicyBuildWorkflowErrorModeled::ResourceInUseException(
                    ResourceInUseException {
                        message: Some(format!(
                            "build workflow {} of automated reasoning policy {} is {}, so \
                             another build workflow cannot be started for it",
                            active.id, input.policy_arn, active.status
                        )),
                    },
                )
                .into(),
            );
        }

        // The policy's workflow quota. Enforcing it here is also what bounds this part of
        // the store: without a server-side cap a caller could grow it with nothing but
        // fresh starts.
        let held = store
            .automated_reasoning_policy_build_workflows_of(&input.policy_arn)
            .count();
        if held >= crate::state::MAX_BUILD_WORKFLOWS_PER_POLICY {
            return Err(
                StartAutomatedReasoningPolicyBuildWorkflowErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: Some(format!(
                            "automated reasoning policy {} already holds the maximum of {} \
                             build workflows; delete one or request a quota increase",
                            input.policy_arn,
                            crate::state::MAX_BUILD_WORKFLOWS_PER_POLICY
                        )),
                    },
                )
                .into(),
            );
        }

        let id = store.mint_build_workflow_id(now);
        let document = source_document(&input.source_content);
        let annotations = source_annotations(&input.source_content);
        // The build runs after this call returns, so the workflow is reported as scheduled
        // now and the start arms every later transition at once: each status of the
        // progression is owed at an instant fixed HERE, on this start's own timeline, so the
        // status the workflow holds is the one the time since this call owes it — a polling
        // caller observes each processing status in turn, and one that waits and reads once
        // is answered with the status that elapsed time owes it.
        let clock = crate::state::AutomatedReasoningPolicyBuildWorkflowClock::armed_at(now);
        let record = crate::state::AutomatedReasoningPolicyBuildWorkflowRecord {
            policy_arn: input.policy_arn.clone(),
            id: id.clone(),
            workflow_type: input.build_workflow_type,
            // A workflow this call has just accepted has not been built yet, and the
            // clock owes it every status of its build.
            status: crate::state::BUILD_WORKFLOW_STATUS_SCHEDULED,
            preprocessing_due_at: Some(clock.preprocessing_at),
            building_due_at: Some(clock.building_at),
            testing_due_at: Some(clock.testing_at),
            completed_due_at: Some(clock.completed_at),
            // Nothing has asked for this workflow to stop, so it is owed no cancellation.
            cancelled_due_at: None,
            document_name: document.map(|document| document.document_name.clone()),
            document_content_type: document
                .map(|document| document.document_content_type.clone()),
            document_description: document.and_then(|document| document.document_description.clone()),
            // The corrections this build was started with are the annotation set
            // `GetAutomatedReasoningPolicyAnnotations` reports for it, so the set is
            // recorded here and the instant it was written with it.
            annotations,
            annotations_updated_at: now,
            created_at: now,
            updated_at: now,
        };

        if let Some(token) = input.client_request_token {
            store
                .automated_reasoning_policy_build_workflow_tokens
                .insert((input.policy_arn.clone(), token), id.clone());
        }
        store
            .automated_reasoning_policy_build_workflows
            .insert((input.policy_arn.clone(), id.clone()), record);

        // Read the response back out of the STORED record, so what the caller is told is
        // exactly what a later GetAutomatedReasoningPolicyBuildWorkflow will find. A
        // record that is not there is answered as a server error rather than by unwinding
        // this request.
        let Some(stored) =
            store.automated_reasoning_policy_build_workflow(&input.policy_arn, &id, now)
        else {
            return Err(server_error());
        };
        Ok(started_workflow(stored))
    }
}

/// The source document a start's `sourceContent` carries, if any: the first entry of
/// whichever document list its `workflowContent` names.
///
/// `AutomatedReasoningPolicyWorkflowTypeContent` carries documents under three of its four
/// members — `documents` (`@length(1, 1)`), the fidelity report content's own `documents`
/// and the iterative refinement content's `documents` — while `policyRepairAssets` carries
/// annotations instead, so a repair names no document
/// (`smithy_query(query="closure",operation="StartAutomatedReasoningPolicyBuildWorkflow")`).
fn source_document(
    source: &AutomatedReasoningPolicyBuildWorkflowSource,
) -> Option<&AutomatedReasoningPolicyBuildWorkflowDocument> {
    match source.workflow_content.as_ref()? {
        AutomatedReasoningPolicyWorkflowTypeContent::Documents(documents) => documents.first(),
        AutomatedReasoningPolicyWorkflowTypeContent::generateFidelityReportContent(
            AutomatedReasoningPolicyGenerateFidelityReportContent::Documents(documents),
        ) => documents.first(),
        AutomatedReasoningPolicyWorkflowTypeContent::iterativeRefinementContent(content) => {
            content.documents.first()
        }
        AutomatedReasoningPolicyWorkflowTypeContent::policyRepairAssets(_) => None,
    }
}

/// The ANNOTATION SET a start's `sourceContent` carries: the annotations of whichever
/// repair content its `workflowContent` names, and the empty set for every other form that
/// member takes.
///
/// `policyRepairAssets` is the ONE member of
/// `AutomatedReasoningPolicyWorkflowTypeContent` that carries annotations — the others
/// carry documents — and its own `annotations` member is `@required`
/// (`smithy_query(query="shape",shape="AutomatedReasoningPolicyBuildWorkflowRepairContent")`),
/// so a repair's set is exactly the list the request sent and a build started from anything
/// else begins with no corrections at all.
fn source_annotations(
    source: &AutomatedReasoningPolicyBuildWorkflowSource,
) -> Vec<AutomatedReasoningPolicyAnnotation> {
    match source.workflow_content.as_ref() {
        Some(AutomatedReasoningPolicyWorkflowTypeContent::policyRepairAssets(repair)) => {
            repair.annotations.clone()
        }
        // Named rather than caught, so a member of this union that comes to carry
        // annotations is a compile error here rather than a silently empty set: every form
        // below carries documents, and a source naming no workflow content carries neither.
        Some(
            AutomatedReasoningPolicyWorkflowTypeContent::Documents(_)
            | AutomatedReasoningPolicyWorkflowTypeContent::generateFidelityReportContent(_)
            | AutomatedReasoningPolicyWorkflowTypeContent::iterativeRefinementContent(_),
        )
        | None => Vec::new(),
    }
}

/// What this operation answers for a stored workflow, so the start and its idempotent
/// replay report ONE record the same way.
fn started_workflow(
    record: &crate::state::AutomatedReasoningPolicyBuildWorkflowRecord,
) -> StartAutomatedReasoningPolicyBuildWorkflowOutput {
    StartAutomatedReasoningPolicyBuildWorkflowOutput {
        policy_arn: record.policy_arn.clone(),
        build_workflow_id: record.id.clone(),
    }
}

/// The modelled 500 this operation answers when the engine carries no store for this
/// service — never a fresh engine, which would drop the workflow on return.
fn server_error() -> StartAutomatedReasoningPolicyBuildWorkflowError {
    StartAutomatedReasoningPolicyBuildWorkflowErrorModeled::InternalServerException(
        InternalServerException {
            message: Some(
                "automated reasoning policy state is not available on this engine".to_string(),
            ),
        },
    )
    .into()
}
