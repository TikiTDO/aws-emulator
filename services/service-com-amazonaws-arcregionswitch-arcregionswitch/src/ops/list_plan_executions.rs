// SPDX-License-Identifier: CC0-1.0

//! @op ListPlanExecutions
//!
//! Reports the executions `StartPlanExecution` recorded against ONE plan — the current
//! one and the historical ones alike — as the `AbbreviatedExecution` projection of each
//! stored record, filtered by `state` when one is given and paged on
//! `maxResults`/`nextToken` as the operation's `@paginated` trait declares. Nothing is
//! built from the request: every member comes from the stored execution.
//!
//! The model documents no order for the list, so this emulator reports the most recently
//! started execution first (the plan's execution log read backwards), which is a total
//! order over the executions of a plan and therefore stable across pages.
//!
//! `@readonly`: the only write is the store's `settle`, which materialises the
//! transitions the clock had already made due — owed at the same instant whether or not
//! anybody called — so that this reader and `GetPlanExecution` never disagree about an
//! execution's state.
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
//! # Modeled contract for `ListPlanExecutions`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")
//! ```
//!
//! Lists the executions of a Region switch plan. This operation returns information about
//! both current and historical executions.
//!
//! ## Input
//!
//! * `maxResults` (ListExecutionsMaxResults, range 1..100, default 100) **required** — The
//!   number of objects that you want to return with this call.
//! * `nextToken` (String) — Specifies that you want to receive the next page of results.
//!   Valid only if you received a nextToken response in the previous request. If you did, it
//!   indicates that more output is available. Set this parameter to the value provided by the
//!   previous call's nextToken response to request the next page of results.
//! * `planArn` (PlanArn, pattern
//!   ^arn:aws[a-zA-Z-]*:arc-region-switch::[0-9]{12}:plan/([a-zA-Z0-9](?:[a-zA-Z0-9-]{0,30}[a-zA-Z0-9])?):([a-z0-9]{6})$)
//!   **required** — The ARN for the plan.
//! * `state` (ExecutionState, one of: canceled | completed |
//!   completedMonitoringApplicationHealth | completedWithExceptions | failed | inProgress |
//!   pausedByFailedStep | pausedByOperator | pending | pendingManualApproval |
//!   planExecutionTimedOut) — The state of the plan execution. For example, the plan
//!   execution might be In Progress.
//!
//! ## Output
//!
//! * `items` (AbbreviatedExecutionsList) — The items in the plan execution to return.
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
// `ListPlanExecutionsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListPlanExecutionsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.arcregionswitch#ListPlanExecutions",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum ListPlanExecutionsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_plan_executions::ListPlanExecutionsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListPlanExecutionsError = crate::scaffolded::Scaffolded<ListPlanExecutionsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListPlanExecutionsErrorModeled> for ListPlanExecutionsError {
    fn from(modeled: ListPlanExecutionsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListPlanExecutions` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListPlanExecutions<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListPlanExecutions<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListPlanExecutions<Eng>
{
    type Input = ListPlanExecutionsInput;
    type Output = ListPlanExecutionsOutput;
    type Error = ListPlanExecutionsError;

    const NAME: &'static str = "ListPlanExecutions";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_PLAN_EXECUTIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_PLAN_EXECUTIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // IAM decides before the resource's state is looked at, so a caller no
        // identity-based policy allows is told only that it is not authorized.
        crate::service::authorization::authorize(&*self.engine, ctx, Self::NAME, &input.plan_arn)
            .map_err(ListPlanExecutionsErrorModeled::AccessDeniedException)?;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(ListPlanExecutionsErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the Region switch plan store is unavailable".to_string(),
                },
            )
            .into());
        };

        // The token is a position in the selected list; the only ones this service hands
        // out are positions strictly inside it, so anything else is not one it issued.
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
        // `maxResults` is `@range(1..100)` with `@default: 100`, so a deserialized input
        // always carries a page size inside that range.
        let page_size = crate::state::page_size(Some(input.max_results));

        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        // `@readonly`: the only write is `settle`, which materialises the transitions the
        // clock already made due — owed at the same instant whether or not anybody called.
        store.settle(now);

        let Some(plan) = store.plans.get(&input.plan_arn) else {
            return Err(ListPlanExecutionsErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: crate::state::plan_not_found_message(&input.plan_arn),
                },
            )
            .into());
        };

        // The plan's execution log is oldest first, so reading it backwards reports the
        // most recently started execution first. `state` selects the executions reporting
        // that `ExecutionState`; an absent one selects every execution of the plan.
        let selected: Vec<&crate::state::StoredExecution> = plan
            .executions
            .iter()
            .rev()
            .filter_map(|execution_id| {
                store
                    .executions
                    .get(&(input.plan_arn.clone(), execution_id.clone()))
            })
            .filter(|execution| {
                input
                    .state
                    .as_deref()
                    .is_none_or(|wanted| execution.execution_state == wanted)
            })
            .collect();

        let total = selected.len();
        // The only offsets this service hands out are 0 (no token at all) and a position
        // strictly inside the selected list, since `nextToken` is set only when a page
        // ends before the last item. Anything at or past the end is no more a token it
        // issued than a non-numeric one, so it is refused rather than answered with an
        // empty page.
        if offset >= total.max(1) {
            return Err(crate::scaffolded::Scaffolded::validation(format!(
                "nextToken is not a pagination token this service issued: it resumes at \
                 {offset}, at or past the last of the {total} executions selected for plan {}",
                input.plan_arn
            )));
        }
        let end = offset.saturating_add(page_size).min(total);
        let items: Vec<AbbreviatedExecution> = selected
            .get(offset..end)
            .unwrap_or_default()
            .iter()
            .map(|execution| execution.as_abbreviated_execution())
            .collect();

        Ok(ListPlanExecutionsOutput {
            items: Some(items),
            next_token: (end < total).then(|| end.to_string()),
        })
    }
}
