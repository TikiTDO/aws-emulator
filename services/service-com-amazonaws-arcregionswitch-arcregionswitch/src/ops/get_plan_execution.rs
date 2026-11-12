// SPDX-License-Identifier: CC0-1.0

//! @op GetPlanExecution
//!
//! Reports the persisted execution: the mode, action, Region, timestamps, step states and
//! plan version `StartPlanExecution` recorded, with the plan itself read back from the
//! store. `stepStates` pages on `maxResults`/`nextToken`, as the operation's
//! `@paginated` trait declares.
//!
//! The only write this reporting operation makes is to materialise the step progression
//! the clock already owed the execution (see
//! [`crate::state::ComAmazonawsArcregionswitchArcregionswitchState::settle`]) — a change
//! that was due at the same instant whether or not anybody called.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. Reach
//! the store through [`crate::state::service_state`] and read it through the guard it
//! hands back. `ctx` ([`smithy_server_runtime::OperationContext`]) carries request
//! metadata only; the engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetPlanExecution`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")
//! ```
//!
//! Retrieves detailed information about a specific plan execution. You must specify the
//! plan ARN and execution ID.
//!
//! ## Input
//!
//! * `executionId` (ExecutionId) **required** — The execution identifier of a plan execution.
//! * `maxResults` (GetPlanExecutionStepStatesMaxResults, range 1..100, default 100)
//!   **required** — The number of objects that you want to return with this call.
//! * `nextToken` (String) — Specifies that you want to receive the next page of results.
//!   Valid only if you received a nextToken response in the previous request. If you did, it
//!   indicates that more output is available. Set this parameter to the value provided by the
//!   previous call's nextToken response to request the next page of results.
//! * `planArn` (PlanArn, pattern
//!   ^arn:aws[a-zA-Z-]*:arc-region-switch::[0-9]{12}:plan/([a-zA-Z0-9](?:[a-zA-Z0-9-]{0,30}[a-zA-Z0-9])?):([a-z0-9]{6})$)
//!   **required** — The Amazon Resource Name (ARN) of the plan with the execution to
//!   retrieve.
//!
//! ## Output
//!
//! * `actualRecoveryTime` (Duration, pattern
//!   ^P(?!$)(\d+Y)?(\d+M)?(\d+D)?(T(?=\d)(\d+H)?(\d+M)?(\d+S)?)?$) — The actual recovery time
//!   that Region switch calculates for a plan execution. Actual recovery time includes the
//!   time for the plan to run added to the time elapsed until the application health alarms
//!   that you've specified are healthy again.
//! * `comment` (String) — A comment included on the plan execution.
//! * `endTime` (Timestamp) — The time (UTC) when the plan execution ended.
//! * `executionAction` (ExecutionAction, one of: activate | deactivate | postRecovery)
//!   **required** — The plan execution action. Valid values are activate, to activate an
//!   Amazon Web Services Region, or deactivate, to deactivate a Region.
//! * `executionId` (ExecutionId) **required** — The execution identifier of a plan execution.
//! * `executionRegion` (String) **required** — The Amazon Web Services Region for a plan
//!   execution.
//! * `executionState` (ExecutionState, one of: canceled | completed |
//!   completedMonitoringApplicationHealth | completedWithExceptions | failed | inProgress |
//!   pausedByFailedStep | pausedByOperator | pending | pendingManualApproval |
//!   planExecutionTimedOut) **required** — The plan execution state. Provides the state of a
//!   plan execution, for example, In Progress or Paused by Operator.
//! * `generatedReportDetails` (GeneratedReportDetails, len 0..1) — Information about the
//!   location of a generated report, or the cause of its failure.
//! * `mode` (ExecutionMode, one of: graceful | ungraceful) **required** — The plan execution
//!   mode. Valid values are graceful, for graceful executions, or ungraceful, for ungraceful
//!   executions.
//! * `nextToken` (String) — Specifies that you want to receive the next page of results.
//!   Valid only if you received a nextToken response in the previous request. If you did, it
//!   indicates that more output is available. Set this parameter to the value provided by the
//!   previous call's nextToken response to request the next page of results.
//! * `plan` (Plan) — The details of the Region switch plan.
//! * `planArn` (PlanArn, pattern
//!   ^arn:aws[a-zA-Z-]*:arc-region-switch::[0-9]{12}:plan/([a-zA-Z0-9](?:[a-zA-Z0-9-]{0,30}[a-zA-Z0-9])?):([a-z0-9]{6})$)
//!   **required** — The Amazon Resource Name (ARN) of the plan.
//! * `recoveryExecutionId` (String) — The unique identifier of the most recent recovery
//!   execution. Required when starting a post-recovery execution.
//! * `startTime` (Timestamp) **required** — The time (UTC) when the plan execution started.
//! * `stepStates` (StepStates) — The states of the steps in the plan execution.
//! * `updatedAt` (Timestamp) — The timestamp when the plan execution was last updated.
//! * `version` (String) — The version for the plan.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient access to perform this
//!   action. HTTP Status Code: 403
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource was not found. HTTP
//!   Status Code: 404

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetPlanExecutionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetPlanExecutionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
// The operation's MODELED errors. `InternalServerException` — a shape the SERVICE models,
// which this operation does not list — is widened in through `extra_errors`, so an
// unreachable shared store has a modelled server failure to answer with.
#[smithy_macros::operation_error(
    "com.amazonaws.arcregionswitch#GetPlanExecution",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum GetPlanExecutionErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_plan_execution::GetPlanExecutionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetPlanExecutionError = crate::scaffolded::Scaffolded<GetPlanExecutionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetPlanExecutionErrorModeled> for GetPlanExecutionError {
    fn from(modeled: GetPlanExecutionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetPlanExecution` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetPlanExecution<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetPlanExecution<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetPlanExecution<Eng>
{
    type Input = GetPlanExecutionInput;
    type Output = GetPlanExecutionOutput;
    type Error = GetPlanExecutionError;

    const NAME: &'static str = "GetPlanExecution";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_PLAN_EXECUTION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_PLAN_EXECUTION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // IAM decides before the resource's state is looked at, so a caller no
        // identity-based policy allows is told only that it is not authorized.
        crate::service::authorization::authorize(&*self.engine, ctx, Self::NAME, &input.plan_arn)
            .map_err(GetPlanExecutionErrorModeled::AccessDeniedException)?;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetPlanExecutionErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the Region switch plan store is unavailable".to_string(),
                },
            )
            .into());
        };

        // `stepStates` pages by position in the execution's step list; the token is that
        // position. A token that is not one is not a token this service ever issued.
        let offset = match input.next_token.as_deref() {
            None => 0usize,
            Some(token) => match token.parse::<usize>() {
                Ok(offset) => offset,
                Err(_) => {
                    return Err(crate::scaffolded::Scaffolded::validation(format!(
                        "nextToken is not a pagination token this service issued: {token}"
                    )))
                }
            },
        };
        let page_size = crate::state::page_size(Some(input.max_results));

        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        // Materialise only what the clock already made due.
        store.settle(now);

        let key = (input.plan_arn.clone(), input.execution_id.clone());
        let Some(execution) = store.executions.get(&key) else {
            return Err(GetPlanExecutionErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: format!(
                        "No execution {} found for plan {}",
                        input.execution_id, input.plan_arn
                    ),
                },
            )
            .into());
        };

        let total = execution.step_states.len();
        // The only offsets this service hands out are 0 (no token at all) and a position
        // strictly inside the step list, since `nextToken` is set only when a page ends
        // before the last step. Anything at or past the end is no more a token it issued
        // than a non-numeric one, so it is refused rather than answered with an empty page.
        if offset >= total.max(1) {
            return Err(crate::scaffolded::Scaffolded::validation(format!(
                "nextToken is not a pagination token this service issued: it resumes at {offset}, \
                 at or past the last of the {total} step states of execution {}",
                input.execution_id
            )));
        }
        let end = offset.saturating_add(page_size).min(total);
        let step_states = execution
            .step_states
            .get(offset..end)
            .unwrap_or_default()
            .to_vec();
        let next_token = (end < total).then(|| end.to_string());

        Ok(GetPlanExecutionOutput {
            plan_arn: execution.plan_arn.clone(),
            execution_id: execution.execution_id.clone(),
            version: Some(execution.plan_version.clone()),
            updated_at: Some(execution.updated_at),
            comment: execution.comment.clone(),
            start_time: execution.start_time,
            end_time: execution.end_time,
            mode: execution.mode.clone(),
            execution_state: execution.execution_state.clone(),
            execution_action: execution.action.clone(),
            execution_region: execution.execution_region.clone(),
            recovery_execution_id: execution.recovery_execution_id.clone(),
            step_states: Some(step_states),
            plan: store
                .plans
                .get(&input.plan_arn)
                .map(crate::state::StoredPlan::as_plan),
            // Nothing ran, so there is no recovery time measured and no report generated.
            actual_recovery_time: None,
            generated_report_details: None,
            next_token,
        })
    }
}
