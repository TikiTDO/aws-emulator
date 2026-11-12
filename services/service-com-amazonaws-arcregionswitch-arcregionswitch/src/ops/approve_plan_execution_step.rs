// SPDX-License-Identifier: CC0-1.0

//! @op ApprovePlanExecutionStep
//!
//! Decides the manual-approval step a plan execution is waiting on, and answers with the
//! empty response the model declares.
//!
//! An `approve` releases the step — it reports `completed`, the execution moves to
//! `inProgress`, and the clock owes it the next step after
//! [`crate::state::EXECUTION_START_SPAN`] — while a `decline` stops the execution:
//! the step is `canceled`, the execution is `canceled` and the plan is released. Only the
//! step the execution is actually waiting on can be decided; any other step of it is the
//! modelled `IllegalStateException` (HTTP 400), and a step the execution does not run at
//! all, or an execution this plan never started, is the modelled
//! `ResourceNotFoundException` (HTTP 404). The whole transition lives in
//! [`crate::state::ComAmazonawsArcregionswitchArcregionswitchState::apply_approval`], so
//! this handler and the clock's own progression cannot disagree about it.
//!
//! A plan that holds no ongoing execution at all has nothing to decide: that is the
//! modelled `ResourceNotFoundException` too, and its message names the plan the request
//! named — the same shape as the ongoing-execution refusals `StartPlanExecution` and
//! `DeletePlan` answer with.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. Reach the
//! store through [`crate::state::service_state`] and read and write it through the guard
//! it hands back. `ctx` ([`smithy_server_runtime::OperationContext`]) carries request
//! metadata only; the engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ApprovePlanExecutionStep`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")
//! ```
//!
//! Approves a step in a plan execution that requires manual approval. When you create a
//! plan, you can include approval steps that require manual intervention before the
//! execution can proceed. This operation allows you to provide that approval. You must
//! specify the plan ARN, execution ID, step name, and approval status. You can also provide
//! an optional comment explaining the approval decision.
//!
//! ## Input
//!
//! * `approval` (Approval, one of: approve | decline) **required** — The status of approval
//!   for a plan execution step.
//! * `comment` (ExecutionComment, len ..1024) — A comment that you can enter about a plan
//!   execution.
//! * `executionId` (ExecutionId) **required** — The execution identifier of a plan execution.
//! * `planArn` (PlanArn, pattern
//!   ^arn:aws[a-zA-Z-]*:arc-region-switch::[0-9]{12}:plan/([a-zA-Z0-9](?:[a-zA-Z0-9-]{0,30}[a-zA-Z0-9])?):([a-z0-9]{6})$)
//!   **required** — The Amazon Resource Name (ARN) of the plan.
//! * `stepName` (StepName) **required** — The name of a step in a plan execution.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient access to perform this
//!   action. HTTP Status Code: 403
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource was not found. HTTP
//!   Status Code: 404

#[allow(unused_imports)]
use super::types::*;

/// The `Approval` value that lets the step proceed, as the model's `Approval` enum
/// declares it (`approve | decline`); the boundary validator has already refused
/// anything else by the time this handler runs.
const APPROVE: &str = "approve";

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ApprovePlanExecutionStepError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json");

// The operation's MODELED errors. Two shapes the SERVICE models, which this operation
// does not list, are widened in through `extra_errors`: `InternalServerException`, for an
// unreachable shared store, and `IllegalStateException` — "the current state of the
// resource doesn't allow the operation to proceed" — for a step that is not waiting for an
// approval decision.
#[smithy_macros::operation_error(
    "com.amazonaws.arcregionswitch#ApprovePlanExecutionStep",
    extra_errors = ["IllegalStateException", "InternalServerException"]
)]
#[derive(Debug)]
pub enum ApprovePlanExecutionStepErrorModeled {
    AccessDeniedException(AccessDeniedException),
    IllegalStateException(IllegalStateException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::approve_plan_execution_step::ApprovePlanExecutionStepError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ApprovePlanExecutionStepError = crate::scaffolded::Scaffolded<ApprovePlanExecutionStepErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ApprovePlanExecutionStepErrorModeled> for ApprovePlanExecutionStepError {
    fn from(modeled: ApprovePlanExecutionStepErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ApprovePlanExecutionStep` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ApprovePlanExecutionStep<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ApprovePlanExecutionStep<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ApprovePlanExecutionStep<Eng>
{
    type Input = ApprovePlanExecutionStepInput;
    type Output = ApprovePlanExecutionStepOutput;
    type Error = ApprovePlanExecutionStepError;

    const NAME: &'static str = "ApprovePlanExecutionStep";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::APPROVE_PLAN_EXECUTION_STEP_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::APPROVE_PLAN_EXECUTION_STEP_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // IAM decides before the resource's state is looked at, so a caller no
        // identity-based policy allows is told only that it is not authorized.
        crate::service::authorization::authorize(&*self.engine, ctx, Self::NAME, &input.plan_arn)
            .map_err(ApprovePlanExecutionStepErrorModeled::AccessDeniedException)?;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                ApprovePlanExecutionStepErrorModeled::InternalServerException(
                    InternalServerException {
                        message: "the Region switch plan store is unavailable".to_string(),
                    },
                )
                .into(),
            );
        };

        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        // Materialise what the clock already made due first, so this decision is applied
        // to the state a reader would see at the same instant — including an approval
        // window that has already closed.
        store.settle(now);

        let key = (input.plan_arn.clone(), input.execution_id.clone());
        let approved = input.approval == APPROVE;
        match store.apply_approval(&key, &input.step_name, approved, input.comment, now) {
            crate::state::ApprovalOutcome::Applied => Ok(ApprovePlanExecutionStepOutput {}),
            crate::state::ApprovalOutcome::NoSuchPlan => Err(
                ApprovePlanExecutionStepErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: format!("Plan not found for ARN: {}", input.plan_arn),
                    },
                )
                .into(),
            ),
            crate::state::ApprovalOutcome::NoOngoingExecution => Err(
                ApprovePlanExecutionStepErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: format!(
                            "There is no active execution ongoing for plan {}",
                            input.plan_arn
                        ),
                    },
                )
                .into(),
            ),
            crate::state::ApprovalOutcome::NoSuchExecution => Err(
                ApprovePlanExecutionStepErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: format!(
                            "No execution {} found for plan {}",
                            input.execution_id, input.plan_arn
                        ),
                    },
                )
                .into(),
            ),
            crate::state::ApprovalOutcome::NoSuchStep => Err(
                ApprovePlanExecutionStepErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: format!(
                            "No step {} found for execution {}",
                            input.step_name, input.execution_id
                        ),
                    },
                )
                .into(),
            ),
            crate::state::ApprovalOutcome::NotAwaitingApproval { status } => Err(
                ApprovePlanExecutionStepErrorModeled::IllegalStateException(
                    IllegalStateException {
                        message: format!(
                            "The step {} of execution {} is {status} and is not waiting for an \
                             approval decision",
                            input.step_name, input.execution_id
                        ),
                    },
                )
                .into(),
            ),
        }
    }
}
