// SPDX-License-Identifier: CC0-1.0

//! @op DeletePlan
//!
//! Removes the plan from the store, together with its executions and the client tokens
//! that started them, so a later `GetPlan` answers `ResourceNotFoundException` and
//! `ListPlans` no longer carries it. A plan still held by an execution that has not
//! reached a state it never leaves answers the modelled `IllegalStateException`
//! (HTTP 400).
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. Reach
//! the store through [`crate::state::service_state`] and read and write it through the
//! guard it hands back. `ctx` ([`smithy_server_runtime::OperationContext`]) carries
//! request metadata only; the engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeletePlan`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")
//! ```
//!
//! Deletes a Region switch plan. You must specify the ARN of the plan to delete. You cannot
//! delete a plan that has an active execution in progress.
//!
//! ## Input
//!
//! * `arn` (PlanArn, pattern
//!   ^arn:aws[a-zA-Z-]*:arc-region-switch::[0-9]{12}:plan/([a-zA-Z0-9](?:[a-zA-Z0-9-]{0,30}[a-zA-Z0-9])?):([a-z0-9]{6})$)
//!   **required** — The Amazon Resource Name (ARN) of the plan.
//!
//! ## Errors
//!
//! * `IllegalStateException` (HTTP 400) — The operation failed because the current state of
//!   the resource doesn't allow the operation to proceed. HTTP Status Code: 400
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource was not found. HTTP
//!   Status Code: 404

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeletePlanError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeletePlanError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
// The operation's MODELED errors. `InternalServerException` — a shape the SERVICE models,
// which this operation does not list — is widened in through `extra_errors`, so an
// unreachable shared store has a modelled server failure to answer with.
#[smithy_macros::operation_error(
    "com.amazonaws.arcregionswitch#DeletePlan",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum DeletePlanErrorModeled {
    IllegalStateException(IllegalStateException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_plan::DeletePlanError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeletePlanError = crate::scaffolded::Scaffolded<DeletePlanErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeletePlanErrorModeled> for DeletePlanError {
    fn from(modeled: DeletePlanErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeletePlan` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeletePlan<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeletePlan<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeletePlan<Eng>
{
    type Input = DeletePlanInput;
    type Output = DeletePlanOutput;
    type Error = DeletePlanError;

    const NAME: &'static str = "DeletePlan";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_PLAN_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_PLAN_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(DeletePlanErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the Region switch plan store is unavailable".to_string(),
                },
            )
            .into());
        };

        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        store.settle(now);

        let Some(record) = store.plans.get(&input.arn) else {
            return Err(DeletePlanErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: crate::state::plan_not_found_message(&input.arn),
                },
            )
            .into());
        };
        let holder = record.active_execution.clone().and_then(|id| {
            store
                .executions
                .get(&(input.arn.clone(), id))
                .filter(|execution| !execution.is_terminal())
        });
        if let Some(holder) = holder {
            // The refusal names what is holding the plan: the plan the request named, and
            // the Region and id of the execution still running against it. An
            // `ExecutionId` is `<region>/<id>` on the wire, so the pair is rendered from
            // its two parts — the Region once, and the BARE id a caller can feed straight
            // back to `GetPlanExecution` or `CancelPlanExecution`.
            let (region, execution_id) = crate::state::execution_id_parts(&holder.execution_id);
            return Err(DeletePlanErrorModeled::IllegalStateException(
                IllegalStateException {
                    message: format!(
                        "Active executions found for {}: {region}: {execution_id}",
                        input.arn
                    ),
                },
            )
            .into());
        }

        // The plan and everything that belongs to it leave the store together, so nothing
        // outlives the plan it was recorded against.
        store.forget_plan(&input.arn);

        Ok(DeletePlanOutput {})
    }
}
