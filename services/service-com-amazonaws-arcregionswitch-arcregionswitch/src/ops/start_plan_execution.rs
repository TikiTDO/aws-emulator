// SPDX-License-Identifier: CC0-1.0

//! @op StartPlanExecution
//!
//! Creates and persists one execution of a stored plan, and answers with the record it
//! stored.
//!
//! The Regions come from the plan and the request, never from the request alone: the
//! model documents `activate` as shifting traffic TO the target Region and `deactivate`
//! as shifting it AWAY from the target Region, and a plan carries exactly two Regions, so
//! one of `activateRegion`/`deactivateRegion` is the target Region and the other is the
//! plan's remaining Region. `postRecovery` shifts no traffic — it follows a recovery
//! execution named by `recoveryExecutionId`, which the model documents as "Required when
//! starting a post-recovery execution" — so it reports neither Region.
//!
//! The execution is stored `pending`, with every step of the workflow the requested
//! action runs recorded `notStarted`, and the clock owes it its first step transition
//! after [`crate::state::EXECUTION_START_SPAN`]. From there the workflow progresses one
//! step at a time in the service state's settle path
//! ([`crate::state::ComAmazonawsArcregionswitchArcregionswitchState::settle`]): a
//! manual-approval step stops the execution at `pendingManualApproval` with that step
//! `pendingApproval`, waiting for `ApprovePlanExecutionStep`, while any other execution
//! block drives other services' resources, which this emulator does not run — so the
//! step fails with [`crate::state::UNSUPPORTED_EXECUTION_REASON`] on an
//! `executionFailed` event and the execution reaches the modelled FAILURE state
//! ([`crate::state::FAILED`]) rather than claiming an outcome nothing produced. A
//! terminal execution releases the plan — it becomes deletable again and can be executed
//! again — rather than wedging it in a run that is not running.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. Reach
//! the store through [`crate::state::service_state`] and read and write it through the
//! guard it hands back. `ctx` ([`smithy_server_runtime::OperationContext`]) carries
//! request metadata only; the engine is NOT reached through `ctx`.
//!
//! ## What a caller has to be allowed to do, and what else observes the start
//!
//! IAM decides FIRST: this operation declares `AccessDeniedException`, and AWS evaluates
//! authorization ahead of any precondition about the plan's own state, so a caller no
//! identity-based policy allows `arc-region-switch:StartPlanExecution` for is refused 403
//! by [`crate::service::authorization`] before the plan is even read.
//!
//! A started execution is also PUBLISHED: the event
//! [`crate::state::execution_events::PlanExecutionEvent`] renders goes onto the account's
//! default EventBridge bus in the execution's Region, so a rule matching this service's
//! source and detail-type delivers it to that rule's targets.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `StartPlanExecution`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")
//! ```
//!
//! Starts the execution of a Region switch plan. You can execute a plan in either graceful
//! or ungraceful mode. Specifing ungraceful mode either changes the behavior of the
//! execution blocks in a workflow or skips specific execution blocks.
//!
//! ## Input
//!
//! * `action` (ExecutionAction, one of: activate | deactivate | postRecovery) **required** —
//!   The action to perform. Valid values are activate (to shift traffic to the target Region)
//!   or deactivate (to shift traffic away from the target Region).
//! * `clientToken` (String, @idempotencyToken) — A unique, case-sensitive identifier to
//!   ensure that the operation completes no more than one time. If this token matches a
//!   previous request, the service ignores the request and returns the result of the original
//!   successful request. If you don't provide a client token, the service automatically
//!   generates one. For more information about idempotency, see Making retries safe with
//!   idempotent APIs.
//! * `comment` (ExecutionComment, len ..1024) — An optional comment explaining why the plan
//!   execution is being started.
//! * `latestVersion` (String) — A boolean value indicating whether to use the latest version
//!   of the plan. If set to false, you must specify a specific version.
//! * `mode` (ExecutionMode, one of: graceful | ungraceful, default "graceful") **required** —
//!   The plan execution mode. Valid values are graceful, for starting the execution in
//!   graceful mode, or ungraceful, for starting the execution in ungraceful mode.
//! * `planArn` (PlanArn, pattern
//!   ^arn:aws[a-zA-Z-]*:arc-region-switch::[0-9]{12}:plan/([a-zA-Z0-9](?:[a-zA-Z0-9-]{0,30}[a-zA-Z0-9])?):([a-z0-9]{6})$)
//!   **required** — The Amazon Resource Name (ARN) of the plan to execute.
//! * `recoveryExecutionId` (RecoveryExecutionId, pattern
//!   ^[a-z]{2}(-[a-z]+)+-[0-9]+/[0-9a-fA-F]{16}$) — The execution identifier of the recovery
//!   execution that ran in the opposite region post-recovery is ran in. Required when
//!   starting a post-recovery execution.
//! * `targetRegion` (String) **required** — The Amazon Web Services Region to target with
//!   this execution. This is the Region that traffic will be shifted to or from, depending on
//!   the action.
//!
//! ## Output
//!
//! * `activateRegion` (String) — The Amazon Web Services Region to activate.
//! * `deactivateRegion` (String) — The Amazon Web Services Region to deactivate.
//! * `executionId` (ExecutionId) — The execution identifier of a plan execution.
//! * `plan` (PlanArn, pattern
//!   ^arn:aws[a-zA-Z-]*:arc-region-switch::[0-9]{12}:plan/([a-zA-Z0-9](?:[a-zA-Z0-9-]{0,30}[a-zA-Z0-9])?):([a-z0-9]{6})$)
//!   — The details of the Region switch plan.
//! * `planVersion` (String) — The version of the plan, a unique number generated by Region
//!   switch.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient access to perform this
//!   action. HTTP Status Code: 403
//! * `ConflictException` (HTTP 409) — The client token was already used with different
//!   request parameters. A client token must map to the same parameters for every request. To
//!   retry this operation, provide a new client token.
//! * `IllegalArgumentException` (HTTP 400) — The request processing has an invalid argument.
//! * `IllegalStateException` (HTTP 400) — The operation failed because the current state of
//!   the resource doesn't allow the operation to proceed. HTTP Status Code: 400
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource was not found. HTTP
//!   Status Code: 404

#[allow(unused_imports)]
use super::types::*;

/// The `ExecutionAction` values that shift traffic, as the model's `ExecutionAction` enum
/// declares them. The third, `postRecovery`, shifts none and is named where the rule about
/// its `recoveryExecutionId` is written
/// ([`crate::state::ACTION_POST_RECOVERY`]).
const ACTION_ACTIVATE: &str = "activate";
const ACTION_DEACTIVATE: &str = "deactivate";

/// The request parameters a `clientToken` is bound to, excluding the token itself.
///
/// The model documents `ConflictException` as "The client token was already used with
/// different request parameters. A client token must map to the same parameters for every
/// request", so the token's first use is recorded against this fingerprint and a later use
/// is a replay only when the fingerprint matches.
fn request_fingerprint(input: &StartPlanExecutionInput) -> String {
    // Length-prefixed, so the encoding is unambiguous however a caller fills the
    // free-form members: `comment` and `latestVersion` may contain ANY byte, separator
    // included, so a plain join would let two distinct parameter sets render to one
    // fingerprint and a different request be answered as a replay.
    let mut fingerprint = String::new();
    for field in [
        input.plan_arn.as_str(),
        input.target_region.as_str(),
        input.action.as_str(),
        input.mode.as_str(),
        input.comment.as_deref().unwrap_or_default(),
        input.latest_version.as_deref().unwrap_or_default(),
        input.recovery_execution_id.as_deref().unwrap_or_default(),
    ] {
        fingerprint.push_str(&field.len().to_string());
        fingerprint.push(':');
        fingerprint.push_str(field);
    }
    fingerprint
}

/// This operation's response, projected from the stored execution record.
fn execution_output(execution: &crate::state::StoredExecution) -> StartPlanExecutionOutput {
    StartPlanExecutionOutput {
        execution_id: Some(execution.execution_id.clone()),
        plan: Some(execution.plan_arn.clone()),
        plan_version: Some(execution.plan_version.clone()),
        activate_region: execution.activate_region.clone(),
        deactivate_region: execution.deactivate_region.clone(),
    }
}

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `StartPlanExecutionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json");

// The operation's MODELED errors. `InternalServerException` — a shape the SERVICE models,
// which this operation does not list — is widened in through `extra_errors`, so an
// unreachable shared store has a modelled server failure to answer with.
#[smithy_macros::operation_error(
    "com.amazonaws.arcregionswitch#StartPlanExecution",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum StartPlanExecutionErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    IllegalArgumentException(IllegalArgumentException),
    IllegalStateException(IllegalStateException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::start_plan_execution::StartPlanExecutionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type StartPlanExecutionError = crate::scaffolded::Scaffolded<StartPlanExecutionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<StartPlanExecutionErrorModeled> for StartPlanExecutionError {
    fn from(modeled: StartPlanExecutionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StartPlanExecution` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct StartPlanExecution<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StartPlanExecution<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StartPlanExecution<Eng>
{
    type Input = StartPlanExecutionInput;
    type Output = StartPlanExecutionOutput;
    type Error = StartPlanExecutionError;

    const NAME: &'static str = "StartPlanExecution";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_PLAN_EXECUTION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_PLAN_EXECUTION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // IAM decides before any precondition about the plan or its executions: a caller
        // no identity-based policy allows is refused here, whatever state the plan is in.
        crate::service::authorization::authorize(&*self.engine, ctx, Self::NAME, &input.plan_arn)
            .map_err(StartPlanExecutionErrorModeled::AccessDeniedException)?;

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(StartPlanExecutionErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the Region switch plan store is unavailable".to_string(),
                },
            )
            .into());
        };

        let now = ::std::time::SystemTime::now();
        // The whole synchronous half runs in this block, so the store guard is RELEASED at
        // its end: what follows is an in-process call this emulator serves, which must not
        // run under this service's lock.
        let (output, event) = {
            let mut store = state.lock();
            store.settle(now);

            // A repeated `clientToken` answers with the execution its first use started;
            // the same token with different parameters is the modelled client-token
            // conflict.
            let fingerprint = request_fingerprint(&input);
            if let Some(token) = input.client_token.as_deref() {
                if let Some(record) = store.client_tokens.get(token) {
                    let key = (record.plan_arn.clone(), record.execution_id.clone());
                    if record.fingerprint != fingerprint {
                        return Err(StartPlanExecutionErrorModeled::ConflictException(
                            ConflictException {
                                message: format!(
                                    "The client token {token} was already used with different \
                                     request parameters"
                                ),
                                resource_id: Some(key.1),
                                resource_type: Some("PlanExecution".to_string()),
                            },
                        )
                        .into());
                    }
                    let Some(existing) = store.executions.get(&key) else {
                        return Err(StartPlanExecutionErrorModeled::InternalServerException(
                            InternalServerException {
                                message: format!(
                                    "the execution {} recorded for client token {token} is \
                                     missing",
                                    key.1
                                ),
                            },
                        )
                        .into());
                    };
                    // Nothing started, so nothing is published: the replay answers with
                    // the execution the FIRST use of the token started.
                    return Ok(execution_output(existing));
                }
            }

            let Some(plan) = store.plans.get(&input.plan_arn) else {
                return Err(StartPlanExecutionErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: crate::state::plan_not_found_message(&input.plan_arn),
                    },
                )
                .into());
            };
            let plan_version = plan.version();
            let plan_regions = plan.plan.regions.clone();
            let counterpart_region = plan.counterpart_region(&input.target_region);
            let active_execution = plan.active_execution.clone();
            let workflows = plan.plan.workflows.clone();

            // A plan names exactly two Regions and an execution shifts traffic between
            // them, so a target outside the plan's Regions is an invalid argument.
            if !plan_regions.contains(&input.target_region) {
                return Err(StartPlanExecutionErrorModeled::IllegalArgumentException(
                    IllegalArgumentException {
                        message: format!(
                            "targetRegion {} is not one of the plan's Regions ({})",
                            input.target_region,
                            plan_regions.join(", ")
                        ),
                    },
                )
                .into());
            }

            // `recoveryExecutionId` is documented "Required when starting a post-recovery
            // execution", in the `region/executionId` form its shape's pattern spells;
            // one reading of that rule, which answers the same sentence for an id that is
            // missing and for one of another form.
            if let Some(rejection) = crate::state::recovery_execution_id_rejection(
                &input.plan_arn,
                &input.action,
                input.recovery_execution_id.as_deref(),
            ) {
                return Err(StartPlanExecutionErrorModeled::IllegalArgumentException(
                    IllegalArgumentException { message: rejection },
                )
                .into());
            }

            // One execution holds the plan until it reaches a state it never leaves.
            let holder = active_execution.filter(|id| {
                store
                    .executions
                    .get(&(input.plan_arn.clone(), id.clone()))
                    .is_some_and(|execution| !execution.is_terminal())
            });
            if holder.is_some() {
                return Err(StartPlanExecutionErrorModeled::IllegalStateException(
                    IllegalStateException {
                        message: format!(
                            "There is already an execution ongoing for plan {}",
                            input.plan_arn
                        ),
                    },
                )
                .into());
            }

            // The execution history this plan keeps is bounded: at the ceiling, the oldest
            // execution that has reached a state it never leaves is dropped, and if none
            // has there is no room for another.
            if !store.room_for_execution(&input.plan_arn) {
                return Err(StartPlanExecutionErrorModeled::IllegalStateException(
                    IllegalStateException {
                        message: format!(
                            "The plan {} already holds the maximum of {} recorded executions, \
                             none of which has finished",
                            input.plan_arn,
                            crate::state::MAX_EXECUTIONS_PER_PLAN
                        ),
                    },
                )
                .into());
            }

            let (activate_region, deactivate_region) = match input.action.as_str() {
                ACTION_ACTIVATE => (
                    Some(input.target_region.clone()),
                    counterpart_region.clone(),
                ),
                ACTION_DEACTIVATE => (
                    counterpart_region.clone(),
                    Some(input.target_region.clone()),
                ),
                // `postRecovery` is the clean-up that follows a recovery: it shifts no
                // traffic, so it neither activates nor deactivates a Region.
                _ => (None, None),
            };

            // The steps of the workflow this action runs, in order and none of them
            // reached yet: the clock, not this call, is what starts the first one.
            // `step_states` is the modelled projection the API reports; `planned_steps` is
            // what the clock needs to decide each transition, captured now so a later edit
            // of the plan cannot change what this execution does.
            let steps: Vec<&Step> = workflows
                .iter()
                .filter(|workflow| workflow.workflow_target_action == input.action)
                .flat_map(|workflow| workflow.steps.iter().flatten())
                .collect();
            let mut step_states: Vec<StepState> = Vec::with_capacity(steps.len());
            let mut planned_steps: Vec<crate::state::PlannedStep> =
                Vec::with_capacity(steps.len());
            for step in steps {
                step_states.push(StepState {
                    name: Some(step.name.clone()),
                    status: Some(crate::state::STEP_NOT_STARTED.to_string()),
                    start_time: None,
                    end_time: None,
                    step_mode: Some(input.mode.clone()),
                });
                planned_steps.push(crate::state::PlannedStep::of(step));
            }

            let seed = store.next_seed();
            let execution_id = crate::state::execution_id(&input.target_region, seed);
            let started_at = ::aws_smithy_types::DateTime::from(now);
            let record = crate::state::StoredExecution {
                plan_arn: input.plan_arn.clone(),
                execution_id: execution_id.clone(),
                plan_version,
                comment: input.comment,
                mode: input.mode,
                action: input.action,
                execution_state: crate::state::PENDING.to_string(),
                execution_region: input.target_region,
                activate_region,
                deactivate_region,
                recovery_execution_id: input.recovery_execution_id,
                start_time: started_at,
                end_time: None,
                updated_at: started_at,
                step_states,
                planned_steps,
                events: Vec::new(),
                outcome_due_at: Some(now + crate::state::EXECUTION_START_SPAN),
                // Nothing has paused it, so there is no state to put it back into and no
                // remainder to serve: both are what `UpdatePlanExecution`'s `pause`
                // records.
                resumes_to: None,
                owed_on_resume: None,
            };

            let key = (input.plan_arn.clone(), execution_id.clone());
            // One entry point for the write, so the plan's execution list, the
            // active-execution link and the clock-owed-transition index cannot drift
            // apart.
            store.insert_execution(record);
            if let Some(token) = input.client_token {
                store.record_client_token(token, fingerprint, key.0.clone(), key.1.clone());
            }

            // Project the response from the STORED record.
            let Some(started) = store.executions.get(&key) else {
                return Err(StartPlanExecutionErrorModeled::InternalServerException(
                    InternalServerException {
                        message: format!(
                            "the execution {} could not be read back after it was stored",
                            key.1
                        ),
                    },
                )
                .into());
            };
            // Starting a switch is published onto the account's default event bus, so a
            // rule matching this service's events delivers it to that rule's targets.
            // Rendered from the stored record, published once the guard is gone.
            (
                execution_output(started),
                crate::state::execution_events::PlanExecutionEvent::of(started),
            )
        };
        event.publish(ctx.callback()).await;
        Ok(output)
    }
}
