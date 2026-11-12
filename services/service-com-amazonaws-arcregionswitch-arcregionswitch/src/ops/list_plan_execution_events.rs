// SPDX-License-Identifier: CC0-1.0

//! @op ListPlanExecutionEvents
//!
//! Reports the event timeline `StartPlanExecution` opened and the clock appended to, in
//! order, oldest first, filtered by `name` when one is given, paged on
//! `maxResults`/`nextToken` as the operation's `@paginated` trait declares. It is the
//! operation that tells a caller WHY an execution reached the state it did: the
//! `executionFailed` event carries the reason in `error`.
//!
//! Implemented here rather than left at 501 because it is the only modelled reader of
//! the reason the execution's outcome carries; the rest of this service's stub census is
//! untouched.
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
//! # Modeled contract for `ListPlanExecutionEvents`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")
//! ```
//!
//! Lists the events that occurred during a plan execution. These events provide a detailed
//! timeline of the execution process.
//!
//! ## Input
//!
//! * `executionId` (ExecutionId) **required** — The execution identifier of a plan execution.
//! * `maxResults` (ListExecutionEventsMaxResults, range 1..100, default 100) **required** —
//!   The number of objects that you want to return with this call.
//! * `name` (StepName) — The name of the plan execution event.
//! * `nextToken` (String) — Specifies that you want to receive the next page of results.
//!   Valid only if you received a nextToken response in the previous request. If you did, it
//!   indicates that more output is available. Set this parameter to the value provided by the
//!   previous call's nextToken response to request the next page of results.
//! * `planArn` (PlanArn, pattern
//!   ^arn:aws[a-zA-Z-]*:arc-region-switch::[0-9]{12}:plan/([a-zA-Z0-9](?:[a-zA-Z0-9-]{0,30}[a-zA-Z0-9])?):([a-z0-9]{6})$)
//!   **required** — The Amazon Resource Name (ARN) of the plan.
//!
//! ## Output
//!
//! * `items` (ExecutionEventList) — The items in the plan execution event.
//! * `nextToken` (String) — Specifies that you want to receive the next page of results.
//!   Valid only if you received a nextToken response in the previous request. If you did, it
//!   indicates that more output is available. Set this parameter to the value provided by the
//!   previous call's nextToken response to request the next page of results.
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
// `ListPlanExecutionEventsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json");

// The operation's MODELED errors. `InternalServerException` — a shape the SERVICE models,
// which this operation does not list — is widened in through `extra_errors`, so an
// unreachable shared store has a modelled server failure to answer with.
#[smithy_macros::operation_error(
    "com.amazonaws.arcregionswitch#ListPlanExecutionEvents",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum ListPlanExecutionEventsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_plan_execution_events::ListPlanExecutionEventsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListPlanExecutionEventsError = crate::scaffolded::Scaffolded<ListPlanExecutionEventsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListPlanExecutionEventsErrorModeled> for ListPlanExecutionEventsError {
    fn from(modeled: ListPlanExecutionEventsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListPlanExecutionEvents` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListPlanExecutionEvents<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListPlanExecutionEvents<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListPlanExecutionEvents<Eng>
{
    type Input = ListPlanExecutionEventsInput;
    type Output = ListPlanExecutionEventsOutput;
    type Error = ListPlanExecutionEventsError;

    const NAME: &'static str = "ListPlanExecutionEvents";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_PLAN_EXECUTION_EVENTS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_PLAN_EXECUTION_EVENTS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // IAM decides before the resource's state is looked at, so a caller no
        // identity-based policy allows is told only that it is not authorized.
        crate::service::authorization::authorize(&*self.engine, ctx, Self::NAME, &input.plan_arn)
            .map_err(ListPlanExecutionEventsErrorModeled::AccessDeniedException)?;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(ListPlanExecutionEventsErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the Region switch plan store is unavailable".to_string(),
                },
            )
            .into());
        };

        // The token is a position in the filtered timeline; the only ones this service
        // hands out are positions strictly inside it.
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
        // `@readonly`: the only write is `settle`, which materialises the outcome the clock
        // already made due — owed at the same instant whether or not anybody called.
        store.settle(now);

        let key = (input.plan_arn.clone(), input.execution_id.clone());
        let Some(execution) = store.executions.get(&key) else {
            return Err(ListPlanExecutionEventsErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: format!(
                        "No execution {} found for plan {}",
                        input.execution_id, input.plan_arn
                    ),
                },
            )
            .into());
        };

        // `name` selects the events of one step; the execution's own lifecycle events
        // carry no step name, so they are not in a filtered timeline.
        let timeline: Vec<&ExecutionEvent> = match input.name.as_deref() {
            None => execution.events.iter().collect(),
            Some(step_name) => execution
                .events
                .iter()
                .filter(|event| event.step_name.as_deref() == Some(step_name))
                .collect(),
        };

        let total = timeline.len();
        if offset >= total.max(1) {
            return Err(crate::scaffolded::Scaffolded::validation(format!(
                "nextToken is not a pagination token this service issued: it resumes at {offset}, \
                 at or past the last of the {total} events of execution {}",
                input.execution_id
            )));
        }
        let end = offset.saturating_add(page_size).min(total);
        let items: Vec<ExecutionEvent> = timeline
            .get(offset..end)
            .unwrap_or_default()
            .iter()
            .map(|event| (*event).clone())
            .collect();

        Ok(ListPlanExecutionEventsOutput {
            items: Some(items),
            next_token: (end < total).then(|| end.to_string()),
        })
    }
}
