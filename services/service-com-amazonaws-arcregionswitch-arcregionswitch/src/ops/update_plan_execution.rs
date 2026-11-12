// SPDX-License-Identifier: CC0-1.0

//! @op UpdatePlanExecution
//!
//! Applies one `UpdatePlanExecutionAction` to a plan execution, and answers with the
//! empty response the model declares.
//!
//! Each action is admitted while the execution is still running, which is what this
//! operation updates: `pause` stops it, `resume` puts a paused one back where it stood,
//! and a mode switch re-points the rest of its workflow. What an action asks for and the
//! execution has already reached changes nothing. Once the execution has reached a state
//! it never leaves there is no in-progress execution to update, so every action is the
//! modelled `IllegalStateException` (HTTP 400), naming the state it was refused from, and
//! a plan or an execution this emulator does not hold is the modelled
//! `ResourceNotFoundException` (HTTP 404). The whole transition lives in
//! [`crate::state::ComAmazonawsArcregionswitchArcregionswitchState::apply_execution_update`],
//! so this handler and the clock's own progression cannot disagree about it.
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
//! # Modeled contract for `UpdatePlanExecution`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")
//! ```
//!
//! Updates an in-progress plan execution. This operation allows you to modify certain
//! aspects of the execution, such as adding a comment or changing the action.
//!
//! ## Input
//!
//! * `action` (UpdatePlanExecutionAction, one of: pause | resume | switchToGraceful |
//!   switchToUngraceful) **required** — The action specified for a plan execution, for
//!   example, Switch to Graceful or Pause.
//! * `comment` (ExecutionComment, len ..1024) — An optional comment about the plan execution.
//! * `executionId` (ExecutionId) **required** — The execution identifier of a plan execution.
//! * `planArn` (PlanArn, pattern
//!   ^arn:aws[a-zA-Z-]*:arc-region-switch::[0-9]{12}:plan/([a-zA-Z0-9](?:[a-zA-Z0-9-]{0,30}[a-zA-Z0-9])?):([a-z0-9]{6})$)
//!   **required** — The Amazon Resource Name (ARN) of the plan with the execution to update.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient access to perform this
//!   action. HTTP Status Code: 403
//! * `IllegalStateException` (HTTP 400) — The operation failed because the current state of
//!   the resource doesn't allow the operation to proceed. HTTP Status Code: 400
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource was not found. HTTP
//!   Status Code: 404

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdatePlanExecutionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json");

// The operation's MODELED errors. `InternalServerException` — a shape the SERVICE models,
// which this operation does not list — is widened in through `extra_errors`, so an
// unreachable shared store has a modelled server failure to answer with.
#[smithy_macros::operation_error(
    "com.amazonaws.arcregionswitch#UpdatePlanExecution",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum UpdatePlanExecutionErrorModeled {
    AccessDeniedException(AccessDeniedException),
    IllegalStateException(IllegalStateException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_plan_execution::UpdatePlanExecutionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdatePlanExecutionError = crate::scaffolded::Scaffolded<UpdatePlanExecutionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdatePlanExecutionErrorModeled> for UpdatePlanExecutionError {
    fn from(modeled: UpdatePlanExecutionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdatePlanExecution` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdatePlanExecution<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdatePlanExecution<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdatePlanExecution<Eng>
{
    type Input = UpdatePlanExecutionInput;
    type Output = UpdatePlanExecutionOutput;
    type Error = UpdatePlanExecutionError;

    const NAME: &'static str = "UpdatePlanExecution";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_PLAN_EXECUTION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_PLAN_EXECUTION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // IAM decides before the resource's state is looked at, so a caller no
        // identity-based policy allows is told only that it is not authorized.
        crate::service::authorization::authorize(&*self.engine, ctx, Self::NAME, &input.plan_arn)
            .map_err(UpdatePlanExecutionErrorModeled::AccessDeniedException)?;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(UpdatePlanExecutionErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the Region switch plan store is unavailable".to_string(),
                },
            )
            .into());
        };

        // The boundary validator refuses a value the `UpdatePlanExecutionAction` enum does
        // not declare before this handler runs, so this is unreachable; it refuses the
        // request rather than answering 501, which would mean "not written yet".
        let Some(update) = crate::state::ExecutionUpdate::of(&input.action) else {
            return Err(crate::scaffolded::Scaffolded::validation(format!(
                "action: {} is not a plan execution action",
                input.action
            )));
        };

        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        // Materialise what the clock already made due first, so this update is applied to
        // the state a reader would see at the same instant — an execution the clock has
        // already finished is not one that can still be paused.
        store.settle(now);

        let key = (input.plan_arn.clone(), input.execution_id.clone());
        match store.apply_execution_update(&key, update, input.comment, now) {
            crate::state::ExecutionUpdateOutcome::Applied => Ok(UpdatePlanExecutionOutput {}),
            crate::state::ExecutionUpdateOutcome::NoSuchPlan => Err(
                UpdatePlanExecutionErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: crate::state::plan_not_found_message(&input.plan_arn),
                    },
                )
                .into(),
            ),
            crate::state::ExecutionUpdateOutcome::NoSuchExecution => Err(
                UpdatePlanExecutionErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: format!(
                            "No execution {} found for plan {}",
                            input.execution_id, input.plan_arn
                        ),
                    },
                )
                .into(),
            ),
            crate::state::ExecutionUpdateOutcome::StateDisallows { state } => Err(
                UpdatePlanExecutionErrorModeled::IllegalStateException(IllegalStateException {
                    message: format!(
                        "The execution {} of plan {} is {state} and cannot be {}",
                        input.execution_id,
                        input.plan_arn,
                        update.effect()
                    ),
                })
                .into(),
            ),
        }
    }
}
