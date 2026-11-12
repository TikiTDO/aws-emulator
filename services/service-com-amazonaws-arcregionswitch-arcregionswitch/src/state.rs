// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-arcregionswitch-arcregionswitch service.
//!
//! Everything a handler persists about a Region switch plan and its executions lives
//! here, behind the engine lock, together with the projections that turn a stored
//! record into the modelled shapes (`Plan`, `AbbreviatedPlan`) the API reports. Handlers
//! never build those shapes from the request: they build them from the record.
//!
//! # Every store here is bounded
//!
//! This process is a developer's emulator that authenticates nobody, so a caller that
//! can reach it must not be able to grow its memory without limit. Each of the stores
//! below has an explicit ceiling and a stated release path, and every insert goes
//! through the method that enforces it:
//!
//! | store | ceiling | release |
//! |---|---|---|
//! | [`ComAmazonawsArcregionswitchArcregionswitchState::plans`] | [`MAX_PLANS`] | `DeletePlan`, or the request is refused |
//! | [`ComAmazonawsArcregionswitchArcregionswitchState::executions`] | [`MAX_EXECUTIONS_PER_PLAN`] per plan | the oldest terminal execution is evicted; deleting the plan drops them all |
//! | [`StoredExecution::events`] | [`MAX_EVENTS_PER_EXECUTION`] per execution | the oldest event is evicted (FIFO) |
//! | [`ComAmazonawsArcregionswitchArcregionswitchState::client_tokens`] | [`MAX_CLIENT_TOKENS`] | the oldest record is evicted (FIFO) |
//! | [`StoredPlan::tags`] | [`MAX_TAGS_PER_PLAN`] | `UntagResource`, or the request is refused |
//!
//! Per-request cost is independent of how much is stored, too: [`settle`] walks only the
//! transitions whose deadline has passed, through the
//! [`ComAmazonawsArcregionswitchArcregionswitchState::due_transitions`] index, never the
//! whole execution map.
//!
//! [`settle`]: ComAmazonawsArcregionswitchArcregionswitchState::settle

/// The EventBridge event a plan execution publishes when it starts.
pub mod execution_events;
/// Whether the role a plan's `executionRole` names is one a plan execution could act as.
pub mod execution_role;

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    AbbreviatedExecution, AbbreviatedPlan, ExecutionBlockConfiguration, ExecutionEvent,
    MinimalWorkflow, Plan, ResourceWarning, Route53HealthCheck, Step, StepState,
};

/// How long after the call that starts one the clock owes an execution its NEXT
/// transition — the first step after `StartPlanExecution`, and the step after the one
/// `ApprovePlanExecutionStep` just released.
///
/// AWS documents no duration for it, so this is the emulator's own choice, inside the
/// project's 250-750 ms band for a transition the clock owes: a caller that reads the
/// execution straight after starting it still observes `pending`, and its state errors.
pub const EXECUTION_START_SPAN: Duration = Duration::from_millis(500);

/// The `ExecutionState` an execution is created in, before the clock has reached its
/// first step, from the model's `ExecutionState` enum
/// (`smithy_query(query="shape", shape="ExecutionState", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`).
pub const PENDING: &str = "pending";

/// The `ExecutionState` of an execution that is between steps: a step has just been
/// decided and the clock has not yet reached the next one.
pub const IN_PROGRESS: &str = "inProgress";

/// The `ExecutionState` of an execution the clock has stopped at a manual-approval step.
///
/// It is NOT terminal: the execution still holds the plan, the step itself reports
/// [`STEP_PENDING_APPROVAL`], and `ApprovePlanExecutionStep` is what moves it on.
pub const PENDING_MANUAL_APPROVAL: &str = "pendingManualApproval";

/// The `ExecutionState` of an execution a caller stopped with `UpdatePlanExecution`'s
/// `pause` action.
///
/// It is NOT terminal: the execution still holds the plan, and the clock owes it nothing
/// while it stands here — a `resume` is the call that puts it back where it stood.
pub const PAUSED_BY_OPERATOR: &str = "pausedByOperator";

/// The `ExecutionState` of an execution stopped at a step that failed. Nothing in this
/// emulator produces it (a step it cannot perform fails the whole execution), but it is
/// the other state a `resume` lifts, so it is named where that rule is written.
pub const PAUSED_BY_FAILED_STEP: &str = "pausedByFailedStep";

/// The `ExecutionState` values a `resume` lifts, from the model's `ExecutionState` enum.
pub const PAUSED_EXECUTION_STATES: &[&str] = &[PAUSED_BY_OPERATOR, PAUSED_BY_FAILED_STEP];

/// The `ExecutionMode` an execution runs in step by step, waiting on each one's own
/// health, from the model's `ExecutionMode` enum
/// (`smithy_query(query="shape", shape="ExecutionMode", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`).
/// It is also `StartPlanExecutionRequest$mode`'s modelled `@default`.
pub const MODE_GRACEFUL: &str = "graceful";

/// The other `ExecutionMode`: the switch is made without waiting on the resources it
/// moves away from.
pub const MODE_UNGRACEFUL: &str = "ungraceful";

/// The `ExecutionState` of an execution every step of which reached
/// [`STEP_COMPLETED`] — including a workflow that configures no execution block at all,
/// which has nothing to perform.
pub const COMPLETED: &str = "completed";

/// The `ExecutionState` of an execution stopped by a declined approval.
pub const CANCELED: &str = "canceled";

/// The `ExecutionState` of an execution whose manual-approval step was neither approved
/// nor declined inside the window its own execution block configures.
pub const PLAN_EXECUTION_TIMED_OUT: &str = "planExecutionTimedOut";

/// The state an execution reaches when the clock gets to a step this emulator cannot
/// perform, from the model's `ExecutionState` enum.
///
/// It is the service's modelled FAILURE state, and it is the honest one: most of a
/// Region switch workflow's execution blocks drive OTHER services' resources (Lambda,
/// ECS, Auto Scaling, RDS, Route 53, ARC routing controls), which this emulator does not
/// run, so nothing performed that step. Reporting `completed` would claim an outcome
/// nothing produced, and staying `inProgress` for ever would report a run that is not
/// running AND never release the plan. Failing with [`UNSUPPORTED_EXECUTION_REASON`]
/// recorded on the execution's events says what happened and frees the plan.
///
/// A manual-approval block is the exception: it drives nothing outside this service, so
/// the clock really does reach it (see [`PENDING_MANUAL_APPROVAL`]).
pub const FAILED: &str = "failed";

/// Why a plan execution fails in this emulator, recorded on the `executionFailed` event
/// `ListPlanExecutionEvents` reports, so a caller is told rather than left guessing.
pub const UNSUPPORTED_EXECUTION_REASON: &str =
    "This emulator does not run a Region switch workflow's execution blocks, which drive \
     other services' resources, so the step this plan execution reached was not performed.";

/// Why an execution waiting for a manual approval ends in [`PLAN_EXECUTION_TIMED_OUT`],
/// recorded the same way.
pub const APPROVAL_TIMEOUT_REASON: &str =
    "The manual-approval step was neither approved nor declined inside the approval \
     window its execution block configures, so the plan execution timed out.";

/// The `StepStatus` of a step the clock has not reached, from the model's `StepStatus`
/// enum
/// (`smithy_query(query="shape", shape="StepStatus", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`).
pub const STEP_NOT_STARTED: &str = "notStarted";

/// The `StepStatus` of a manual-approval step the clock has reached and that is waiting
/// for `ApprovePlanExecutionStep`.
pub const STEP_PENDING_APPROVAL: &str = "pendingApproval";

/// The `StepStatus` of a step that is done: a manual approval that was approved.
pub const STEP_COMPLETED: &str = "completed";

/// The `StepStatus` of a step that reached a negative outcome of its own: one this
/// emulator could not perform, and one whose manual approval was DECLINED.
pub const STEP_FAILED: &str = "failed";

/// The `StepStatus` of a step stopped from outside itself rather than by an outcome of its
/// own: its approval window closed before anybody decided it.
pub const STEP_CANCELED: &str = "canceled";

/// The `ExecutionEventType` recorded when an execution is created.
pub const EVENT_EXECUTION_PENDING: &str = "executionPending";

/// The `ExecutionEventType` recorded when an execution reaches [`FAILED`] or
/// [`PLAN_EXECUTION_TIMED_OUT`]; the event's `error` says which it was and why. The
/// model's `ExecutionEventType` enum declares no timed-out member, so this is the
/// nearest it has.
pub const EVENT_EXECUTION_FAILED: &str = "executionFailed";

/// The `ExecutionEventType` recorded when the clock stops at a manual-approval step.
pub const EVENT_STEP_PENDING_APPROVAL: &str = "stepPendingApproval";

/// The `ExecutionEventType` recorded when a manual-approval step is approved.
pub const EVENT_STEP_SUCCEEDED: &str = "stepSucceeded";

/// The `ExecutionEventType` recorded when an execution reaches [`COMPLETED`].
pub const EVENT_EXECUTION_SUCCEEDED: &str = "executionSucceeded";

/// The `ExecutionEventType` recorded when a declined approval stops an execution.
pub const EVENT_EXECUTION_CANCELED: &str = "executionCanceled";

/// The `ExecutionEventType` recorded when a caller pauses an execution.
pub const EVENT_EXECUTION_PAUSED: &str = "executionPaused";

/// The `ExecutionEventType` recorded when the clock STARTS an execution: it leaves
/// [`PENDING`] and the workflow is being driven, which is also what a `resume` of a paused
/// execution does. One event type for both, because both are the same fact about the
/// execution — it is running now — and it is the only start event the timeline exposes, so
/// a caller reconstructing an execution's span reads it as the instant work began.
pub const EVENT_EXECUTION_STARTED: &str = "executionStarted";

/// The `ExecutionEventType` recorded when an execution is switched to [`MODE_GRACEFUL`].
pub const EVENT_BEHAVIOR_CHANGED_TO_GRACEFUL: &str = "executionBehaviorChangedToGraceful";

/// The `ExecutionEventType` recorded when an execution is switched to
/// [`MODE_UNGRACEFUL`].
pub const EVENT_BEHAVIOR_CHANGED_TO_UNGRACEFUL: &str = "executionBehaviorChangedToUngraceful";

/// The approval window this emulator falls back to, in minutes.
///
/// It is the model's own default for the member that carries it
/// (`ExecutionApprovalConfiguration$timeoutMinutes`, "The timeout value specified for
/// the configuration", `@range(min: 1)`, `@default: 60`), so a stored block that somehow
/// carries no usable value waits exactly as long as one that spells the default out.
pub const DEFAULT_APPROVAL_TIMEOUT_MINUTES: u64 = 60;

/// The Region a request that carries no SigV4 credential scope is attributed to.
/// Matches the Region generated emulators default to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `Route53HealthCheckStatus` this emulator reports for a health check it lists,
/// from the model's `Route53HealthCheckStatus` enum
/// (`smithy_query(query="shape", shape="Route53HealthCheckStatus", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`).
///
/// Nothing in this emulator probes a Route 53 record's endpoint, so no health state has
/// been observed for it: `unknown` is the modelled value for exactly that. Reporting
/// `healthy` or `unhealthy` would claim an observation nothing made.
pub const HEALTH_CHECK_STATUS_UNKNOWN: &str = "unknown";

/// The `EvaluationStatus` of a plan whose stored configuration raised no warning, from
/// the model's `EvaluationStatus` enum
/// (`smithy_query(query="shape", shape="EvaluationStatus", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`).
pub const EVALUATION_PASSED: &str = "passed";

/// The `EvaluationStatus` of a plan carrying at least one active warning: part of the
/// plan has to be corrected before a switch can shift traffic.
pub const EVALUATION_ACTION_REQUIRED: &str = "actionRequired";

/// The `ResourceWarningStatus` of a warning the plan's CURRENT version still raises,
/// from the model's `ResourceWarningStatus` enum
/// (`smithy_query(query="shape", shape="ResourceWarningStatus", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`).
///
/// Every warning this emulator reports is derived from the stored plan, so a warning the
/// configuration no longer raises is not reported at all; `resolved` would claim a
/// history of warnings that nothing records.
pub const WARNING_STATUS_ACTIVE: &str = "active";

/// The most plans this emulator stores. Not an AWS quota — AWS publishes its own, which
/// this does not claim to reproduce — but the ceiling that keeps an unauthenticated
/// caller from growing the store without limit. `CreatePlan` refuses past it, and
/// `DeletePlan` is the release path.
pub const MAX_PLANS: usize = 500;

/// The most executions this emulator keeps per plan. Reaching it evicts the oldest
/// execution that has reached a state it never leaves; if none has, `StartPlanExecution`
/// is refused. (One plan can hold only one non-terminal execution at a time, so this
/// ceiling is what bounds the history that accumulates behind it.)
pub const MAX_EXECUTIONS_PER_PLAN: usize = 50;

/// The most events this emulator keeps on one execution's timeline. Reaching it drops the
/// oldest event, so a caller that keeps updating one execution cannot grow the store
/// without limit; it is far above the event count a workflow's own steps produce.
pub const MAX_EVENTS_PER_EXECUTION: usize = 1_000;

/// The most tags one plan carries.
///
/// AWS documents this one: `TagResourceRequest$tags` reads "Tags that you add to a
/// resource. You can add a maximum of 50 tags in Region switch."
/// (`smithy_query(query="doc", shape="TagResourceRequest", member="tags", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`),
/// so it is the service's own limit rather than an emulator choice. The `Tags` map's
/// `@length(0..200)` bounds ONE request; this bounds what a plan accumulates over many,
/// and it also bounds the store, since tags are the one part of a plan record a caller
/// can keep growing after the plan exists.
pub const MAX_TAGS_PER_PLAN: usize = 50;

/// `TagKey`'s modelled minimum length (`@length(min: 1, max: 128)`), from
/// `smithy_query(query="shape", shape="TagKey", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`.
pub const TAG_KEY_MIN_LENGTH: usize = 1;

/// `TagKey`'s modelled maximum length, from the same shape.
pub const TAG_KEY_MAX_LENGTH: usize = 128;

/// `TagValue`'s modelled maximum length (`@length(min: 0, max: 256)`), from
/// `smithy_query(query="shape", shape="TagValue", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`.
///
/// Nothing here enforces it: the boundary validator reaches a map's VALUE target, so a
/// tag value past this length is refused before any handler runs. It is stated for the
/// tests that pin that bound.
pub const TAG_VALUE_MAX_LENGTH: usize = 256;

/// Why `key` is not an acceptable tag key, naming the member and the constraint it
/// breaks, or `None` when it is acceptable.
///
/// The boundary validator reaches a map's VALUE target but not its KEY target, so a
/// request carrying a tag key outside `TagKey`'s `@length` reaches the handler. This is
/// where that constraint is enforced, for every operation that takes tag keys.
pub fn tag_key_rejection(member: &str, key: &str) -> Option<String> {
    let length = key.chars().count();
    (!(TAG_KEY_MIN_LENGTH..=TAG_KEY_MAX_LENGTH).contains(&length)).then(|| {
        format!(
            "{member}: a tag key is {TAG_KEY_MIN_LENGTH} to {TAG_KEY_MAX_LENGTH} characters \
             long; {key:?} is {length}"
        )
    })
}

/// Why this set of tag keys is not acceptable, or `None` when every one of them is.
pub fn tag_keys_rejection<'a>(
    member: &str,
    keys: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    keys.into_iter()
        .find_map(|key| tag_key_rejection(member, key))
}

/// The most `clientToken` records this emulator keeps. Reaching it evicts the oldest
/// recorded token, so the idempotency cache cannot accumulate without bound; a caller
/// whose token was evicted is answered as a fresh request rather than a replay.
pub const MAX_CLIENT_TOKENS: usize = 2_048;

/// The `ExecutionState` values an execution never leaves, from the model's
/// `ExecutionState` enum
/// (`smithy_query(query="shape", shape="ExecutionState", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`).
/// Every other value — `pending`, `inProgress`, `pausedByFailedStep`,
/// `pausedByOperator`, `pendingManualApproval` — is a state the execution can still
/// move out of, so it still holds the plan.
pub const TERMINAL_EXECUTION_STATES: &[&str] = &[
    CANCELED,
    COMPLETED,
    "completedMonitoringApplicationHealth",
    "completedWithExceptions",
    FAILED,
    PLAN_EXECUTION_TIMED_OUT,
];

/// The two Regions of a plan name one switch between them, so they have to be two
/// DIFFERENT Regions: `regions` is `@length(min: 2, max: 2)` (the boundary validator
/// refuses any other count before a handler runs) and an execution shifts traffic from
/// one of them to the other, which the same Region twice describes no such shift of.
///
/// `Some(message)` is the refusal for a handler that takes a plan's Region pair — today
/// `CreatePlan`, and `UpdatePlan` once it is written.
///
/// The sentence is the service's own, and it is FIXED text: it states what is wrong with
/// the plan and what to do about it, and interpolates nothing — the Region named twice is
/// in the request the caller sent, so the refusal quotes no value back.
pub fn region_pair_violation(regions: &[String]) -> Option<String> {
    let mut seen: BTreeSet<&String> = BTreeSet::new();
    regions.iter().find(|region| !seen.insert(region))?;
    Some(DUPLICATE_REGIONS_MESSAGE.to_string())
}

/// What a request naming the same Region twice in a plan's `regions` is refused with.
///
/// Fixed text, trailing space included: that is the sentence the service emits, and a
/// client that branches on it or localizes it matches it exactly.
pub const DUPLICATE_REGIONS_MESSAGE: &str =
    "There are duplicate regions defined in the plan. Please remove the duplicated regions. ";

/// Why the `recoveryExecutionId` of a `postRecovery` execution of the plan `plan_arn` is
/// not usable, or `None` when the request carries one in the documented form (and for
/// every other action, which needs none).
///
/// `recoveryExecutionId` is documented "Required when starting a post-recovery execution"
/// and `RecoveryExecutionId` is `region/executionId` (pattern
/// `^[a-z]{2}(-[a-z]+)+-[0-9]+/[0-9a-fA-F]{16}$`), so a post-recovery execution that
/// names none and one that names a value of another form are the SAME refusal: the id the
/// request has to carry is not there. One sentence for both, naming the plan the request
/// addressed and the form to supply.
pub fn recovery_execution_id_rejection(
    plan_arn: &str,
    action: &str,
    recovery_execution_id: Option<&str>,
) -> Option<String> {
    if action != ACTION_POST_RECOVERY {
        return None;
    }
    if recovery_execution_id.is_some_and(is_recovery_execution_id) {
        return None;
    }
    Some(format!(
        "The recoveryExecutionId is missing or incorrectly formatted for the {plan_arn}. \
         Please provide the recoveryExecutionId in the format 'region/executionId' for Post \
         Recovery execution."
    ))
}

/// The `ExecutionAction` that follows a recovery execution, from the model's
/// `ExecutionAction` enum
/// (`smithy_query(query="shape", shape="ExecutionAction", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`).
pub const ACTION_POST_RECOVERY: &str = "postRecovery";

/// Whether `value` is a `RecoveryExecutionId`: `region/executionId`, as the shape's
/// pattern `^[a-z]{2}(-[a-z]+)+-[0-9]+/[0-9a-fA-F]{16}$` spells it — a Region label
/// followed by a 16-digit hexadecimal execution id.
fn is_recovery_execution_id(value: &str) -> bool {
    let Some((region, id)) = value.split_once('/') else {
        return false;
    };
    id.len() == 16 && id.bytes().all(|byte| byte.is_ascii_hexdigit()) && is_region_label(region)
}

/// Whether `value` is a Region label of the form `RecoveryExecutionId`'s pattern requires:
/// two lowercase letters, one or more lowercase-letter segments, and a trailing number.
fn is_region_label(value: &str) -> bool {
    let mut segments = value.split('-');
    let Some(first) = segments.next() else {
        return false;
    };
    if first.len() != 2 || !first.bytes().all(|byte| byte.is_ascii_lowercase()) {
        return false;
    }
    let rest: Vec<&str> = segments.collect();
    let Some((number, middle)) = rest.split_last() else {
        return false;
    };
    !middle.is_empty()
        && middle
            .iter()
            .all(|segment| !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_lowercase()))
        && !number.is_empty()
        && number.bytes().all(|byte| byte.is_ascii_digit())
}

/// The `ResourceNotFoundException` message for a plan ARN this emulator cannot resolve a
/// plan from, built from the ARN the REQUEST carried.
///
/// One function so every operation that looks a plan up by ARN renders that refusal
/// identically — `GetPlan`, `DeletePlan`, `StartPlanExecution`,
/// `ListRoute53HealthChecks`, and the Region-scoped reads, which answer the same shape
/// and the same message at [`IN_REGION_NOT_FOUND_STATUS`] instead of the status the
/// shape's `@httpError` names.
pub fn plan_not_found_message(arn: &str) -> String {
    format!("Plan not found for ARN: {arn}")
}

/// The HTTP status a Region-scoped (`...InRegion`) read answers a plan `arn` it cannot
/// resolve from the Region the call reached with.
///
/// `ResourceNotFoundException` carries `@httpError 404`, and 404 is what the
/// Region-agnostic reads of a plan answer with. The recorded responses of this service's
/// Region-scoped reads carry that same shape, `__type` and message at HTTP 400: from one
/// Region's endpoint an ARN that names no plan available there is answered as a plain
/// client error. Only the status is the service's own — everything on the wire stays the
/// modelled shape's.
pub const IN_REGION_NOT_FOUND_STATUS: u16 = 400;

/// The page size a paginated operation of this service serves.
///
/// `ListPlans`, `GetPlanExecution`, `ListPlanExecutionEvents` and `ListPlanExecutions`
/// all constrain `maxResults` to `@range(1..100)`; an absent one asks for as many as
/// that range allows. One helper so the rule is written once.
pub fn page_size(requested: Option<i32>) -> usize {
    const MAX_PAGE_SIZE: i32 = 100;
    let clamped = requested.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
    usize::try_from(clamped).unwrap_or(1)
}

/// A Region switch plan as this emulator persists it: the modelled resource plus the
/// bookkeeping that is not part of the `Plan` shape.
#[derive(Debug, Clone)]
pub struct StoredPlan {
    /// The plan as the API reports it. `version` is NOT kept here — it is derived from
    /// [`StoredPlan::revision`], so there is one source of truth for it.
    pub plan: Plan,
    /// How many times the plan has been written. Rendered as the plan's `version`.
    pub revision: u64,
    /// The tags supplied for the plan, kept for `ListTagsForResource`/`TagResource`.
    pub tags: BTreeMap<String, String>,
    /// The ids of the executions started against this plan, oldest first. Bounded by
    /// [`MAX_EXECUTIONS_PER_PLAN`].
    pub executions: Vec<String>,
    /// The execution currently holding the plan, while it is in a non-terminal state.
    pub active_execution: Option<String>,
}

impl StoredPlan {
    /// The plan's `version`, derived from how many times it has been written.
    pub fn version(&self) -> String {
        self.revision.to_string()
    }

    /// The `Plan` shape for this record.
    pub fn as_plan(&self) -> Plan {
        let mut plan = self.plan.clone();
        plan.version = Some(self.version());
        plan
    }

    /// The `AbbreviatedPlan` projection `ListPlans` reports.
    pub fn as_abbreviated_plan(&self) -> AbbreviatedPlan {
        AbbreviatedPlan {
            arn: self.plan.arn.clone(),
            owner: self.plan.owner.clone(),
            name: self.plan.name.clone(),
            regions: self.plan.regions.clone(),
            recovery_approach: self.plan.recovery_approach.clone(),
            primary_region: self.plan.primary_region.clone(),
            version: Some(self.version()),
            updated_at: self.plan.updated_at,
            description: self.plan.description.clone(),
            execution_role: Some(self.plan.execution_role.clone()),
            active_plan_execution: self.active_execution.clone(),
            recovery_time_objective_minutes: self.plan.recovery_time_objective_minutes,
        }
    }

    /// Whether this plan is available in `region`.
    ///
    /// A plan's `regions` are the Amazon Web Services Regions it covers (`RegionList`,
    /// len 2..2), and `ListPlansInRegion` is documented to list the plans "available in
    /// the current Amazon Web Services Region" — so a plan is available in exactly the
    /// Regions its own `regions` names. The Region-scoped reads decide with this, through
    /// [`ComAmazonawsArcregionswitchArcregionswitchState::plan_available_in_region`].
    pub fn is_available_in_region(&self, region: &str) -> bool {
        self.plan.regions.iter().any(|r| r == region)
    }

    /// The plan's other Region: a plan carries exactly two (`RegionList`, len 2..2), so
    /// the Region that is not `region` is unique when `region` is one of them.
    pub fn counterpart_region(&self, region: &str) -> Option<String> {
        self.plan.regions.iter().find(|r| *r != region).cloned()
    }

    /// The Route 53 health checks this plan configures, as
    /// `ListRoute53HealthChecks` reports them: one per (hosted zone, record name,
    /// Region) triple any `Route53HealthCheck` execution block of its workflows names,
    /// deduplicated and in ascending order of that triple so paging over them is stable.
    ///
    /// The hosted zone and the record name come from the block's own `hostedZoneId` and
    /// `recordName`. The REGIONS are the Regions the block's `recordSets` name — and when
    /// it names none, the plan's own two Regions: `region` is a required member of
    /// `Route53HealthCheck`, a Region switch shifts traffic between both of the plan's
    /// Regions, and the record the block names is reached in each of them, so a block that
    /// singles out no record set configures a health check for that record in each Region
    /// the plan covers. A plan whose workflows carry no such block configures none.
    ///
    /// `healthCheckId` is absent and `status` is [`HEALTH_CHECK_STATUS_UNKNOWN`]: no
    /// Route 53 health check resource was created for the record — the id a caller would
    /// read back from Route 53 is Route 53's to mint, and this emulator reports none it
    /// does not hold — and nothing probed the record's endpoint.
    pub fn route53_health_checks(&self) -> Vec<Route53HealthCheck> {
        let mut found: BTreeSet<(String, String, String)> = BTreeSet::new();
        for workflow in &self.plan.workflows {
            collect_route53_health_checks(
                workflow.steps.as_deref().unwrap_or_default(),
                &self.plan.regions,
                &mut found,
            );
        }
        found
            .into_iter()
            .map(
                |(hosted_zone_id, record_name, region)| Route53HealthCheck {
                    hosted_zone_id,
                    record_name,
                    health_check_id: None,
                    status: Some(HEALTH_CHECK_STATUS_UNKNOWN.to_string()),
                    region,
                },
            )
            .collect()
    }

    /// The plan's current evaluation warnings, as `GetPlanEvaluationStatus` reports them:
    /// one active [`ResourceWarning`] for the plan's `executionRole` when that role cannot
    /// be acted as, one for every workflow step whose execution block leaves one of the
    /// plan's Regions unconfigured, and one for every regional resource it configures in a
    /// Region the plan does not cover.
    ///
    /// `execution_role_refusal` is why the plan's `executionRole` cannot be acted as, from
    /// [`execution_role::execution_role_refusal`] — the caller resolves it, because the
    /// role lives in ANOTHER service's store and this projection reads only the plan. A
    /// plan execution performs its workflows as that role, so a plan whose role does not
    /// resolve cannot execute at all, which is the first thing an evaluation has to say
    /// about it. That warning is about the PLAN rather than about one step, so it names
    /// neither a workflow nor a step and carries the role's ARN as its `resourceArn`.
    ///
    /// A plan covers exactly two Regions (`regions` is `@length(min: 2, max: 2)`) and a
    /// switch shifts traffic between them in either direction, so a step that drives
    /// regional resources needs one configured in EACH of them. Every execution block that
    /// drives such resources says which Region each one is for — a Region-keyed map
    /// (`RegionAuroraClusterMap`, `RdsDbInstanceArnMap`, `RegionEventSourceMappingMap`,
    /// `RegionSsmDocumentMap`, `RegionAndRoutingControls`, `RegionAuroraInstanceArnMap`), a
    /// resource ARN whose fourth segment is its Region, or a record set's own `region` —
    /// and several of those maps are modelled `@length(min: 1)`, so a configuration naming
    /// only one of the two Regions passes validation and is exactly what an evaluation has
    /// to flag. A block that drives nothing regional (a manual approval, a nested plan) has
    /// no Region to leave out and raises nothing.
    ///
    /// The warnings are derived from the stored plan on every read, so they are a function
    /// of the record: [`ResourceWarning::version`] is the plan version they were derived
    /// from and `warningUpdatedTime` is when that version was written, which is when the
    /// configuration last changed.
    pub fn evaluation_warnings(
        &self,
        execution_role_refusal: Option<String>,
    ) -> Vec<ResourceWarning> {
        let version = self.version();
        let updated_at = self.plan.updated_at.unwrap_or(DateTime::from_secs(0));
        let covered: BTreeSet<&str> = self.plan.regions.iter().map(String::as_str).collect();
        let regions = self.plan.regions.join(", ");
        let mut warnings = Vec::new();
        if let Some(warning_message) = execution_role_refusal {
            warnings.push(ResourceWarning {
                workflow: None,
                version: version.clone(),
                step_name: None,
                resource_arn: Some(self.plan.execution_role.clone()),
                warning_status: WARNING_STATUS_ACTIVE.to_string(),
                warning_updated_time: updated_at,
                warning_message,
            });
        }
        for workflow in &self.plan.workflows {
            // A stored workflow carries no `name` member, so the warning names it the only
            // two ways the record can: the action it performs and its own description.
            let origin = MinimalWorkflow {
                action: Some(workflow.workflow_target_action.clone()),
                name: workflow.workflow_description.clone(),
            };
            for step in flattened_steps(workflow.steps.as_deref().unwrap_or_default()) {
                let resources = regional_resources(&step.execution_block_configuration);
                if resources.is_empty() {
                    continue;
                }
                let block = &step.execution_block_type;
                let named: BTreeSet<&str> =
                    resources.iter().map(|r| r.region.as_str()).collect();
                for region in &self.plan.regions {
                    if named.contains(region.as_str()) {
                        continue;
                    }
                    warnings.push(ResourceWarning {
                        workflow: Some(origin.clone()),
                        version: version.clone(),
                        step_name: Some(step.name.clone()),
                        resource_arn: None,
                        warning_status: WARNING_STATUS_ACTIVE.to_string(),
                        warning_updated_time: updated_at,
                        warning_message: format!(
                            "The {block} execution block of step {} configures no resource in \
                             {region}. A Region switch shifts traffic between both of the \
                             plan's Regions ({regions}), so the step needs a resource \
                             configured in each of them.",
                            step.name
                        ),
                    });
                }
                for resource in resources.iter().filter(|r| !covered.contains(r.region.as_str()))
                {
                    let subject = match resource.arn.as_deref() {
                        Some(arn) => format!("the resource {arn}"),
                        None => "a resource".to_string(),
                    };
                    warnings.push(ResourceWarning {
                        workflow: Some(origin.clone()),
                        version: version.clone(),
                        step_name: Some(step.name.clone()),
                        resource_arn: resource.arn.clone(),
                        warning_status: WARNING_STATUS_ACTIVE.to_string(),
                        warning_updated_time: updated_at,
                        warning_message: format!(
                            "The {block} execution block of step {} configures {subject} in {}, \
                             which is not one of the plan's Regions ({regions}).",
                            step.name, resource.region
                        ),
                    });
                }
            }
        }
        warnings
    }
}

/// The `EvaluationStatus` of a plan whose evaluation produced `warnings`: a plan with an
/// active warning needs a correction, and one with none passed.
///
/// It is decided from EVERY warning of the plan, never from one page of them.
pub fn evaluation_state(warnings: &[ResourceWarning]) -> String {
    if warnings
        .iter()
        .any(|warning| warning.warning_status == WARNING_STATUS_ACTIVE)
    {
        EVALUATION_ACTION_REQUIRED.to_string()
    } else {
        EVALUATION_PASSED.to_string()
    }
}

/// One regional resource an execution block configures: the Region it is configured for,
/// and the ARN identifying it when the configuration records one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RegionalResource {
    region: String,
    arn: Option<String>,
}

impl RegionalResource {
    /// The resource an ARN names, or `None` when the ARN names no Region — a global
    /// resource (an IAM role, a Region switch plan) is not configured for a Region, so it
    /// is not evidence that any Region is covered.
    fn of_arn(arn: &str) -> Option<Self> {
        Some(Self {
            region: arn_region(arn)?.to_string(),
            arn: Some(arn.to_string()),
        })
    }

    /// The resource a Region-keyed configuration entry names: the Region is the key the
    /// configuration files it under, whatever Region its own ARN names.
    fn of_region(region: &str, arn: Option<&str>) -> Self {
        Self {
            region: region.to_string(),
            arn: arn.map(str::to_string),
        }
    }
}

/// The Region an ARN names, or `None` when its Region segment is empty.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>`, so the Region is
/// its fourth colon-separated segment; a global resource leaves that segment empty.
fn arn_region(arn: &str) -> Option<&str> {
    arn.split(':').nth(3).filter(|region| !region.is_empty())
}

/// The regional resources one execution block configures, sorted and deduplicated so the
/// warnings derived from them are in the same order on every read (several of the
/// configurations hold their entries in a hash map).
///
/// A block that drives nothing regional yields none: a manual approval waits inside this
/// service, a nested Region switch plan is named by a Region-less ARN, and a parallel
/// block holds only the steps walked separately by [`flattened_steps`].
fn regional_resources(config: &ExecutionBlockConfiguration) -> Vec<RegionalResource> {
    let mut resources: Vec<RegionalResource> = match config {
        ExecutionBlockConfiguration::customActionLambdaConfig(config) => config
            .lambdas
            .iter()
            .filter_map(|lambda| RegionalResource::of_arn(lambda.arn.as_deref()?))
            .collect(),
        ExecutionBlockConfiguration::ec2AsgCapacityIncreaseConfig(config) => config
            .asgs
            .iter()
            .filter_map(|asg| RegionalResource::of_arn(asg.arn.as_deref()?))
            .collect(),
        ExecutionBlockConfiguration::ecsCapacityIncreaseConfig(config) => config
            .services
            .iter()
            .flat_map(|service| [service.service_arn.as_deref(), service.cluster_arn.as_deref()])
            .flatten()
            .filter_map(RegionalResource::of_arn)
            .collect(),
        ExecutionBlockConfiguration::eksResourceScalingConfig(config) => config
            .eks_clusters
            .iter()
            .flatten()
            .filter_map(|cluster| RegionalResource::of_arn(&cluster.cluster_arn))
            .collect(),
        ExecutionBlockConfiguration::globalAuroraConfig(config) => config
            .database_cluster_arns
            .iter()
            .filter_map(|arn| RegionalResource::of_arn(arn))
            .collect(),
        ExecutionBlockConfiguration::documentDbConfig(config) => config
            .database_cluster_arns
            .iter()
            .filter_map(|arn| RegionalResource::of_arn(arn))
            .collect(),
        ExecutionBlockConfiguration::arcRoutingControlConfig(config) => config
            .region_and_routing_controls
            .iter()
            .flat_map(|(region, controls)| {
                // A Region with no routing control still names the Region it is
                // configured for.
                if controls.is_empty() {
                    return vec![RegionalResource::of_region(region, None)];
                }
                controls
                    .iter()
                    .map(|control| {
                        RegionalResource::of_region(region, Some(&control.routing_control_arn))
                    })
                    .collect()
            })
            .collect(),
        ExecutionBlockConfiguration::lambdaEventSourceMappingConfig(config) => config
            .region_event_source_mappings
            .iter()
            .map(|(region, mapping)| RegionalResource::of_region(region, Some(&mapping.arn)))
            .collect(),
        ExecutionBlockConfiguration::auroraServerlessScalingConfig(config) => config
            .region_database_cluster_arns
            .iter()
            .map(|(region, arn)| RegionalResource::of_region(region, Some(arn)))
            .collect(),
        ExecutionBlockConfiguration::auroraProvisionedScalingConfig(config) => config
            .region_database_cluster_arns
            .iter()
            .chain(config.instance_arns.iter())
            .map(|(region, arn)| RegionalResource::of_region(region, Some(arn)))
            .collect(),
        ExecutionBlockConfiguration::neptuneGlobalDatabaseConfig(config) => config
            .region_database_cluster_arns
            .iter()
            .map(|(region, arn)| RegionalResource::of_region(region, Some(arn)))
            .collect(),
        ExecutionBlockConfiguration::rdsPromoteReadReplicaConfig(config) => config
            .db_instance_arn_map
            .iter()
            .map(|(region, arn)| RegionalResource::of_region(region, Some(arn)))
            .collect(),
        ExecutionBlockConfiguration::rdsCreateCrossRegionReadReplicaConfig(config) => config
            .db_instance_arn_map
            .iter()
            .map(|(region, arn)| RegionalResource::of_region(region, Some(arn)))
            .collect(),
        ExecutionBlockConfiguration::rdsSwitchoverReadReplicaConfig(config) => config
            .db_instance_arn_map
            .iter()
            .map(|(region, arn)| RegionalResource::of_region(region, Some(arn)))
            .collect(),
        ExecutionBlockConfiguration::ssmSendCommandConfig(config) => config
            .region_documents
            .iter()
            .map(|(region, document)| {
                RegionalResource::of_region(region, Some(&document.document_arn))
            })
            .collect(),
        ExecutionBlockConfiguration::route53HealthCheckConfig(config) => config
            .record_sets
            .iter()
            .flatten()
            .filter_map(|record_set| {
                Some(RegionalResource::of_region(
                    record_set.region.as_deref()?,
                    None,
                ))
            })
            .collect(),
        ExecutionBlockConfiguration::executionApprovalConfig(_)
        | ExecutionBlockConfiguration::regionSwitchPlanConfig(_)
        | ExecutionBlockConfiguration::parallelConfig(_) => Vec::new(),
    };
    resources.sort();
    resources.dedup();
    resources
}

/// Every step `steps` holds, in document order, descending into the steps a `Parallel`
/// block nests — each nested step is a step of the workflow and is evaluated as one.
///
/// The walk keeps its own stack rather than recursing: how deeply `parallelConfig` nests
/// is chosen by whoever created the plan, so recursion would let one request grow the
/// call stack.
fn flattened_steps(steps: &[Step]) -> Vec<&Step> {
    let mut flattened = Vec::new();
    let mut pending: Vec<&Step> = steps.iter().rev().collect();
    while let Some(step) = pending.pop() {
        flattened.push(step);
        if let ExecutionBlockConfiguration::parallelConfig(nested) =
            &step.execution_block_configuration
        {
            pending.extend(nested.steps.iter().rev());
        }
    }
    flattened
}

/// Collect the `(hostedZoneId, recordName, region)` triples the `Route53HealthCheck`
/// blocks reachable from `steps` name, descending into the steps a `Parallel` block
/// nests.
///
/// A block that singles out no record set is taken to configure the record it names in
/// each of `plan_regions`, the Regions the plan covers: `region` is required on the
/// reported health check, and a switch moves the record's traffic between exactly those
/// Regions.
///
/// The walk keeps its own stack rather than recursing: how deeply `parallelConfig`
/// nests is chosen by whoever created the plan, so recursion would let one request
/// grow the call stack.
fn collect_route53_health_checks(
    steps: &[Step],
    plan_regions: &[String],
    found: &mut BTreeSet<(String, String, String)>,
) {
    let mut pending: Vec<&Step> = steps.iter().collect();
    while let Some(step) = pending.pop() {
        match &step.execution_block_configuration {
            ExecutionBlockConfiguration::parallelConfig(nested) => {
                pending.extend(nested.steps.iter());
            }
            ExecutionBlockConfiguration::route53HealthCheckConfig(config) => {
                let named: Vec<&String> = config
                    .record_sets
                    .iter()
                    .flatten()
                    .filter_map(|record_set| record_set.region.as_ref())
                    .collect();
                let regions: Vec<&String> = if named.is_empty() {
                    plan_regions.iter().collect()
                } else {
                    named
                };
                for region in regions {
                    found.insert((
                        config.hosted_zone_id.clone(),
                        config.record_name.clone(),
                        region.clone(),
                    ));
                }
            }
            _ => {}
        }
    }
}

/// What the clock needs to know about one step of the workflow an execution runs,
/// captured when the execution is created so a later edit of the plan cannot change what
/// a running execution does.
///
/// It sits beside the modelled [`StepState`] projection (same order, same length): the
/// projection is what the API reports, this is what decides the next transition.
#[derive(Debug, Clone)]
pub struct PlannedStep {
    /// The step's name, as `StepState$name` reports it.
    pub name: String,
    /// `Some(window)` when the step is a manual-approval execution block, and the window
    /// its configuration gives the approval; `None` for every other block type, which
    /// drives another service's resources and so is one this emulator cannot perform.
    pub approval_window: Option<Duration>,
}

impl PlannedStep {
    /// The clock's plan for one step of a stored workflow.
    pub fn of(step: &Step) -> Self {
        let approval_window = match &step.execution_block_configuration {
            // A manual approval is the one execution block whose work is inside THIS
            // service: it waits for `ApprovePlanExecutionStep` and drives nothing else.
            ExecutionBlockConfiguration::executionApprovalConfig(config) => {
                Some(approval_window(config.timeout_minutes))
            }
            _ => None,
        };
        Self {
            name: step.name.clone(),
            approval_window,
        }
    }
}

/// How long a manual-approval step waits, from the block's own configuration.
///
/// AWS documents the window on the block itself
/// (`ExecutionApprovalConfiguration$timeoutMinutes`: "The timeout value specified for the
/// configuration", `@range(min: 1)`, `@default: 60`), so the configured value is what is
/// waited — this is not a duration the emulator chooses. A value the model does not admit
/// falls back to its documented default.
fn approval_window(timeout_minutes: i32) -> Duration {
    let minutes = u64::try_from(timeout_minutes)
        .ok()
        .filter(|minutes| *minutes >= 1)
        .unwrap_or(DEFAULT_APPROVAL_TIMEOUT_MINUTES);
    Duration::from_secs(minutes * 60)
}

/// One execution of a plan, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct StoredExecution {
    pub plan_arn: String,
    pub execution_id: String,
    /// The plan `version` this execution was started against.
    pub plan_version: String,
    pub comment: Option<String>,
    pub mode: String,
    pub action: String,
    pub execution_state: String,
    pub execution_region: String,
    pub activate_region: Option<String>,
    pub deactivate_region: Option<String>,
    pub recovery_execution_id: Option<String>,
    pub start_time: DateTime,
    pub end_time: Option<DateTime>,
    pub updated_at: DateTime,
    pub step_states: Vec<StepState>,
    /// What the clock needs to know about each of those steps, in the same order.
    pub planned_steps: Vec<PlannedStep>,
    /// The execution's events, oldest first, as `ListPlanExecutionEvents` reports them,
    /// at most [`MAX_EVENTS_PER_EXECUTION`] of them: a caller that keeps asking for
    /// updates (pausing and resuming, say) appends events after the workflow's own, so
    /// the timeline has a ceiling and drops its oldest event at it.
    pub events: Vec<ExecutionEvent>,
    /// When the clock owes this execution its next transition; `None` once it owes none —
    /// because the execution has reached a state it never leaves, or because it is waiting
    /// for a call (an approval) rather than for the clock.
    pub outcome_due_at: Option<SystemTime>,
    /// The `ExecutionState` a `pause` interrupted, so a `resume` puts the execution back
    /// where it stood instead of guessing a state for it; `None` whenever the execution is
    /// not paused.
    pub resumes_to: Option<String>,
    /// What the clock still owed the execution when a `pause` stopped it — the rest of a
    /// manual-approval window, say. A `resume` serves that remainder from the instant it
    /// is called, so a pause costs the execution no progress and takes none away either;
    /// `None` when the clock owed it nothing, or the execution is not paused.
    pub owed_on_resume: Option<Duration>,
}

impl StoredExecution {
    /// Whether the execution has reached a state it never leaves, so the plan is free.
    pub fn is_terminal(&self) -> bool {
        TERMINAL_EXECUTION_STATES.contains(&self.execution_state.as_str())
    }

    /// Whether the execution stands in a state a `resume` lifts.
    pub fn is_paused(&self) -> bool {
        PAUSED_EXECUTION_STATES.contains(&self.execution_state.as_str())
    }

    /// The position of the step the execution is waiting for an approval decision on.
    pub fn step_awaiting_approval(&self) -> Option<usize> {
        self.step_states
            .iter()
            .position(|step| step.status.as_deref() == Some(STEP_PENDING_APPROVAL))
    }

    /// The position of the first step the clock has not reached yet.
    pub fn next_unreached_step(&self) -> Option<usize> {
        self.step_states
            .iter()
            .position(|step| step.status.as_deref() == Some(STEP_NOT_STARTED))
    }

    /// The position of the step called `name`, whatever state it is in.
    pub fn step_named(&self, name: &str) -> Option<usize> {
        self.step_states
            .iter()
            .position(|step| step.name.as_deref() == Some(name))
    }

    /// The `AbbreviatedExecution` projection `ListPlanExecutions` reports: the execution
    /// record without the per-step detail, the plan and the report `GetPlanExecution`
    /// adds.
    ///
    /// `actualRecoveryTime` is absent for the same reason it is absent from
    /// `GetPlanExecution`: no execution block ran, so nothing measured a recovery time.
    pub fn as_abbreviated_execution(&self) -> AbbreviatedExecution {
        AbbreviatedExecution {
            plan_arn: self.plan_arn.clone(),
            execution_id: self.execution_id.clone(),
            version: Some(self.plan_version.clone()),
            updated_at: Some(self.updated_at),
            comment: self.comment.clone(),
            start_time: self.start_time,
            end_time: self.end_time,
            mode: self.mode.clone(),
            execution_state: self.execution_state.clone(),
            execution_action: self.action.clone(),
            execution_region: self.execution_region.clone(),
            recovery_execution_id: self.recovery_execution_id.clone(),
            actual_recovery_time: None,
        }
    }
}

/// What a `clientToken` recorded, so a replay of `StartPlanExecution` returns the first
/// execution and a reuse with different parameters is refused.
#[derive(Debug, Clone)]
pub struct TokenRecord {
    /// The request parameters the token was first used with.
    pub fingerprint: String,
    pub plan_arn: String,
    pub execution_id: String,
    /// Where this record sits in the FIFO that bounds the token cache.
    pub recorded_at: u64,
}

/// The ONE transition the clock owes an execution, decided from the step it stands on.
enum Advance {
    /// Stop at the manual-approval step at this position, for the window given.
    HoldForApproval { index: usize, window: Duration },
    /// The step at this position is an execution block this emulator cannot perform.
    UnsupportedStep(usize),
    /// The approval window of the step at this position has closed undecided.
    ApprovalWindowClosed(usize),
    /// No step is left to reach.
    WorkflowDone,
}

/// What [`ComAmazonawsArcregionswitchArcregionswitchState::apply_approval`] found when it
/// tried to decide a step, so `ApprovePlanExecutionStep` can answer the right modelled
/// failure for each case.
#[derive(Debug)]
pub enum ApprovalOutcome {
    /// The decision was applied.
    Applied,
    /// This emulator holds no plan under that ARN.
    NoSuchPlan,
    /// The plan is ongoing no execution at all, so it has nothing to decide: every
    /// execution it ever started has reached a state it never leaves.
    NoOngoingExecution,
    /// This plan holds no execution with that id.
    NoSuchExecution,
    /// The execution runs no step of that name.
    NoSuchStep,
    /// The step is not waiting for a decision; it reports `status`.
    NotAwaitingApproval { status: String },
}

/// What `UpdatePlanExecution` asks of an execution, parsed from the modelled
/// `UpdatePlanExecutionAction` the request carries
/// (`smithy_query(query="shape", shape="UpdatePlanExecutionAction", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`:
/// `pause | resume | switchToGraceful | switchToUngraceful`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionUpdate {
    /// Stop the clock on the execution, recording what it still owed.
    Pause,
    /// Put a paused execution back where it stood and let the clock drive it again.
    Resume,
    /// Run the rest of the workflow in this [`ExecutionMode`][MODE_GRACEFUL].
    SwitchMode(&'static str),
}

impl ExecutionUpdate {
    /// The update one modelled `UpdatePlanExecutionAction` value asks for; `None` for a
    /// value the enum does not declare, which the boundary validator has already refused.
    pub fn of(action: &str) -> Option<Self> {
        match action {
            "pause" => Some(Self::Pause),
            "resume" => Some(Self::Resume),
            "switchToGraceful" => Some(Self::SwitchMode(MODE_GRACEFUL)),
            "switchToUngraceful" => Some(Self::SwitchMode(MODE_UNGRACEFUL)),
            _ => None,
        }
    }

    /// What this update would do to the execution, for the message of a refusal that
    /// names the state it was refused from.
    pub fn effect(&self) -> String {
        match self {
            Self::Pause => "paused".to_string(),
            Self::Resume => "resumed".to_string(),
            Self::SwitchMode(mode) => format!("switched to {mode} mode"),
        }
    }
}

/// What [`ComAmazonawsArcregionswitchArcregionswitchState::apply_execution_update`] found
/// when it tried to apply one, so `UpdatePlanExecution` can answer the right modelled
/// failure for each case.
#[derive(Debug)]
pub enum ExecutionUpdateOutcome {
    /// The update was admitted, and whatever it had to change is changed.
    Applied,
    /// This emulator holds no plan under that ARN.
    NoSuchPlan,
    /// That plan holds no execution with that id.
    NoSuchExecution,
    /// The execution reports `state`, which it never leaves: there is no in-progress
    /// execution left to update.
    StateDisallows { state: String },
}

/// Per-service state for com-amazonaws-arcregionswitch-arcregionswitch. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsArcregionswitchArcregionswitchState {
    /// Plans by ARN, at most [`MAX_PLANS`] of them. A `BTreeMap` so `ListPlans` pages in
    /// a stable order and a token can simply name the ARN to resume from.
    pub plans: BTreeMap<String, StoredPlan>,
    /// Executions by `(plan ARN, executionId)`, at most [`MAX_EXECUTIONS_PER_PLAN`] per
    /// plan.
    pub executions: BTreeMap<(String, String), StoredExecution>,
    /// `clientToken` -> the execution it started, at most [`MAX_CLIENT_TOKENS`] of them.
    pub client_tokens: BTreeMap<String, TokenRecord>,
    /// The FIFO order of `client_tokens`, as `recorded_at -> clientToken`, so the oldest
    /// record can be evicted in log time instead of by scanning the cache.
    client_token_order: BTreeMap<u64, String>,
    /// The clock-owed transitions still outstanding, as
    /// `(due instant, plan ARN, executionId)`. Sorted by deadline, so [`Self::settle`]
    /// touches only what is actually due.
    due_transitions: BTreeSet<(SystemTime, String, String)>,
    /// Monotonic seed for the ids this service mints and for the token FIFO.
    seed: u64,
}

impl ComAmazonawsArcregionswitchArcregionswitchState {
    /// The next value of this store's monotonic id seed.
    pub fn next_seed(&mut self) -> u64 {
        self.seed += 1;
        self.seed
    }

    /// The plan a Region-scoped (`...InRegion`) read's `arn` names, resolved from the
    /// Region the call reached: `None` when no plan is stored under that ARN, and when
    /// the plan stored under it does not cover `region`.
    ///
    /// One function so every Region-scoped read resolves its argument the same way. Both
    /// `None` cases are one refusal — from that Region's endpoint there is no such plan —
    /// so both answer [`plan_not_found_message`] at [`IN_REGION_NOT_FOUND_STATUS`].
    pub fn plan_available_in_region(&self, arn: &str, region: &str) -> Option<&StoredPlan> {
        self.plans
            .get(arn)
            .filter(|record| record.is_available_in_region(region))
    }

    /// Apply the state changes the clock already made due at `now`.
    ///
    /// Only clock-owed work: a transition that was owed at `now` even if nobody had
    /// called. EVERY handler runs this first — the reporting ones included, which is
    /// what keeps them from disagreeing with the mutating ones about what the current
    /// state is — and it costs only the transitions that are due, never a walk of the
    /// store.
    ///
    /// The transitions are the workflow's own progression, one step at a time (see
    /// [`Self::advance_execution`]): the clock reaches a step, and the step either stops
    /// the execution for a manual approval, or is one this emulator cannot perform and
    /// fails it. A transition that lands another one in the past (a long-expired approval
    /// window, say) is applied in the same pass, which is why this loop re-reads the
    /// index rather than taking a snapshot of it.
    pub fn settle(&mut self, now: SystemTime) {
        while let Some(next) = self.due_transitions.pop_first() {
            if next.0 > now {
                self.due_transitions.insert(next);
                break;
            }
            let (due, plan_arn, execution_id) = next;
            self.advance_execution(&(plan_arn, execution_id), due);
        }
    }

    /// Apply the ONE transition the clock owed `key` at `due`.
    ///
    /// An execution the clock reaches while it is still [`PENDING`] is STARTING: the
    /// workflow is being driven from here on, so an [`EVENT_EXECUTION_STARTED`] event
    /// anchors the timeline before the first event of the step this transition reaches.
    /// Once — a resume of an execution paused before it ever started has already recorded
    /// that the execution is running, and an execution starts only the one time.
    ///
    /// The step the execution stands on decides which transition it is:
    ///
    /// * a step waiting for a manual approval whose window has closed →
    ///   [`PLAN_EXECUTION_TIMED_OUT`];
    /// * the next step not reached yet, when it IS a manual-approval block →
    ///   [`PENDING_MANUAL_APPROVAL`], and the clock then owes the window's end;
    /// * the next step not reached yet, when it is any other block → [`FAILED`], because
    ///   that block drives another service's resources and nothing performed it;
    /// * no step left to reach → [`COMPLETED`].
    fn advance_execution(&mut self, key: &(String, String), due: SystemTime) {
        let at = DateTime::from(due);
        let Some(execution) = self.executions.get(key) else {
            return;
        };
        let starting = execution.execution_state == PENDING
            && !execution
                .events
                .iter()
                .any(|event| event.r#type.as_deref() == Some(EVENT_EXECUTION_STARTED));
        let next = if execution.is_terminal() {
            // Nothing is owed to an execution that has reached a state it never leaves;
            // reaching here means a stale deadline outlived it, so it is dropped rather
            // than allowed to move a settled outcome.
            self.schedule(key, None);
            return;
        } else if let Some(index) = execution.step_awaiting_approval() {
            Advance::ApprovalWindowClosed(index)
        } else if let Some(index) = execution.next_unreached_step() {
            match execution
                .planned_steps
                .get(index)
                .and_then(|planned| planned.approval_window)
            {
                Some(window) => Advance::HoldForApproval { index, window },
                None => Advance::UnsupportedStep(index),
            }
        } else {
            // Every step of the workflow is done — or the workflow configures no
            // execution block at all, which has nothing to perform. Either way no
            // outcome is claimed for work nothing ran, because no work is left.
            Advance::WorkflowDone
        };

        // The execution was pending and the clock has now reached it: it is running, and
        // the event that says so precedes every event of the work this transition does.
        if starting {
            self.push_event(key, EVENT_EXECUTION_STARTED, None, at);
        }

        match next {
            Advance::HoldForApproval { index, window } => {
                let name = self.mark_step(key, index, STEP_PENDING_APPROVAL, at, false);
                if let Some(execution) = self.executions.get_mut(key) {
                    execution.execution_state = PENDING_MANUAL_APPROVAL.to_string();
                    execution.updated_at = at;
                }
                self.push_detailed_event(key, EVENT_STEP_PENDING_APPROVAL, name, None, None, at);
                // The clock owes the window's end, not the outcome: until then this is a
                // call (`ApprovePlanExecutionStep`) the execution is waiting for.
                self.schedule(key, Some(due + window));
            }
            Advance::UnsupportedStep(index) => {
                self.mark_step(key, index, STEP_FAILED, at, true);
                if let Some(execution) = self.executions.get_mut(key) {
                    execution.execution_state = FAILED.to_string();
                    execution.updated_at = at;
                    execution.end_time = Some(at);
                }
                self.push_event(
                    key,
                    EVENT_EXECUTION_FAILED,
                    Some(UNSUPPORTED_EXECUTION_REASON.to_string()),
                    at,
                );
                self.finish_execution(key);
            }
            Advance::ApprovalWindowClosed(index) => {
                let name = self.mark_step(key, index, STEP_CANCELED, at, true);
                if let Some(execution) = self.executions.get_mut(key) {
                    execution.execution_state = PLAN_EXECUTION_TIMED_OUT.to_string();
                    execution.updated_at = at;
                    execution.end_time = Some(at);
                }
                self.push_detailed_event(
                    key,
                    EVENT_EXECUTION_FAILED,
                    name,
                    None,
                    Some(APPROVAL_TIMEOUT_REASON.to_string()),
                    at,
                );
                self.finish_execution(key);
            }
            Advance::WorkflowDone => {
                if let Some(execution) = self.executions.get_mut(key) {
                    execution.execution_state = COMPLETED.to_string();
                    execution.updated_at = at;
                    execution.end_time = Some(at);
                }
                self.push_event(key, EVENT_EXECUTION_SUCCEEDED, None, at);
                self.finish_execution(key);
            }
        }
    }

    /// Apply a manual-approval decision to one step of one execution.
    ///
    /// The decision applies to the execution the plan is ONGOING, and to the step that
    /// execution is actually waiting on. An execution that has reached a state it never
    /// leaves is past deciding, so once the plan is ongoing none there is nothing to
    /// decide at all ([`ApprovalOutcome::NoOngoingExecution`]) — which is what a caller
    /// deciding a step twice runs into when the first decision finished the execution.
    ///
    /// Approving releases the step — it reports [`STEP_COMPLETED`], the execution
    /// [`IN_PROGRESS`], and the clock owes the next step after [`EXECUTION_START_SPAN`],
    /// so a caller reading straight after the approval still sees the execution between
    /// steps. Declining stops the execution: the step reached a negative outcome OF ITS
    /// OWN, so it reports [`STEP_FAILED`] while the execution reports [`CANCELED`], and
    /// the plan is released. [`STEP_CANCELED`] stays what it says — a step stopped from
    /// outside itself, which is what the approval window closing undecided does to one.
    pub fn apply_approval(
        &mut self,
        key: &(String, String),
        step_name: &str,
        approved: bool,
        comment: Option<String>,
        now: SystemTime,
    ) -> ApprovalOutcome {
        let Some(plan) = self.plans.get(&key.0) else {
            return ApprovalOutcome::NoSuchPlan;
        };
        let ongoing = plan.active_execution.clone().filter(|execution_id| {
            self.executions
                .get(&(key.0.clone(), execution_id.clone()))
                .is_some_and(|execution| !execution.is_terminal())
        });
        let Some(ongoing) = ongoing else {
            return ApprovalOutcome::NoOngoingExecution;
        };
        if ongoing != key.1 {
            return ApprovalOutcome::NoSuchExecution;
        }
        let Some(execution) = self.executions.get(key) else {
            return ApprovalOutcome::NoSuchExecution;
        };
        let Some(index) = execution.step_named(step_name) else {
            return ApprovalOutcome::NoSuchStep;
        };
        if execution.step_awaiting_approval() != Some(index) {
            let status = execution
                .step_states
                .get(index)
                .and_then(|step| step.status.clone())
                .unwrap_or_else(|| STEP_NOT_STARTED.to_string());
            return ApprovalOutcome::NotAwaitingApproval { status };
        }

        let at = DateTime::from(now);
        if approved {
            let name = self.mark_step(key, index, STEP_COMPLETED, at, true);
            if let Some(execution) = self.executions.get_mut(key) {
                execution.execution_state = IN_PROGRESS.to_string();
                execution.updated_at = at;
            }
            self.push_detailed_event(key, EVENT_STEP_SUCCEEDED, name, comment, None, at);
            self.schedule(key, Some(now + EXECUTION_START_SPAN));
        } else {
            // The decline is the step's OWN negative outcome: the step failed, and the
            // execution it was holding is canceled. Two distinct modelled `StepStatus`
            // values, so a caller reading `stepStates` can tell the step that ended the
            // execution by an operator's decline from one an external cancel stopped.
            let name = self.mark_step(key, index, STEP_FAILED, at, true);
            if let Some(execution) = self.executions.get_mut(key) {
                execution.execution_state = CANCELED.to_string();
                execution.updated_at = at;
                execution.end_time = Some(at);
            }
            self.push_detailed_event(key, EVENT_EXECUTION_CANCELED, name, comment, None, at);
            self.finish_execution(key);
        }
        ApprovalOutcome::Applied
    }

    /// Apply one `UpdatePlanExecution` action to one execution of one plan.
    ///
    /// The operation updates an IN-PROGRESS execution, so the one precondition is that
    /// the execution has not ended: one that has reached a state it never leaves is
    /// [`ExecutionUpdateOutcome::StateDisallows`] — the modelled `IllegalStateException`
    /// — for every action, because there is no longer an execution to update. While it is
    /// still running the action is admitted, and what it changes is decided by the state
    /// the execution stands in:
    ///
    /// * [`ExecutionUpdate::Pause`] stops it at [`PAUSED_BY_OPERATOR`], remembering the
    ///   state it stood in and what the clock still owed it. One already paused has no
    ///   progress left to stop, so nothing changes.
    /// * [`ExecutionUpdate::Resume`] puts a paused one ([`PAUSED_EXECUTION_STATES`]) back
    ///   into the state the pause interrupted, with the clock owing it the remainder it
    ///   had left, measured from this call. One that is not paused is not waiting on a
    ///   resume, so nothing changes.
    /// * [`ExecutionUpdate::SwitchMode`] re-points it, and every step the clock has NOT
    ///   reached, at the given [`ExecutionMode`][MODE_GRACEFUL]; a step already reached
    ///   keeps the mode it ran in. One already running that mode has nothing to switch.
    ///
    /// An action that changes nothing appends no event: nothing happened to record. A
    /// `comment` the request carries is what the execution reports from then on either
    /// way, and is recorded on the event an action that DID change something appends.
    pub fn apply_execution_update(
        &mut self,
        key: &(String, String),
        update: ExecutionUpdate,
        comment: Option<String>,
        now: SystemTime,
    ) -> ExecutionUpdateOutcome {
        if !self.plans.contains_key(&key.0) {
            return ExecutionUpdateOutcome::NoSuchPlan;
        }
        let Some(execution) = self.executions.get(key) else {
            return ExecutionUpdateOutcome::NoSuchExecution;
        };
        if execution.is_terminal() {
            return ExecutionUpdateOutcome::StateDisallows {
                state: execution.execution_state.clone(),
            };
        }
        let state = execution.execution_state.clone();
        let at = DateTime::from(now);

        let event = match update {
            ExecutionUpdate::Pause if !execution.is_paused() => {
                // Read what the clock owed before the deadline is dropped, so the
                // remainder a resume serves is the one that was outstanding.
                let owed = execution
                    .outcome_due_at
                    .map(|due| due.duration_since(now).unwrap_or_default());
                self.schedule(key, None);
                if let Some(execution) = self.executions.get_mut(key) {
                    execution.resumes_to = Some(state);
                    execution.owed_on_resume = owed;
                    execution.execution_state = PAUSED_BY_OPERATOR.to_string();
                }
                Some(EVENT_EXECUTION_PAUSED)
            }
            ExecutionUpdate::Resume if execution.is_paused() => {
                // A state the pause did not record (a step the service paused, which
                // nothing here produces) resumes as an execution between steps, with the
                // clock owing it the next one.
                let resumed = execution
                    .resumes_to
                    .clone()
                    .unwrap_or_else(|| IN_PROGRESS.to_string());
                let owed = execution.owed_on_resume.unwrap_or(EXECUTION_START_SPAN);
                if let Some(execution) = self.executions.get_mut(key) {
                    execution.execution_state = resumed;
                    execution.resumes_to = None;
                    execution.owed_on_resume = None;
                }
                self.schedule(key, Some(now + owed));
                Some(EVENT_EXECUTION_STARTED)
            }
            ExecutionUpdate::SwitchMode(mode) if execution.mode != mode => {
                if let Some(execution) = self.executions.get_mut(key) {
                    execution.mode = mode.to_string();
                    for step in &mut execution.step_states {
                        if step.status.as_deref() == Some(STEP_NOT_STARTED) {
                            step.step_mode = Some(mode.to_string());
                        }
                    }
                }
                Some(if mode == MODE_UNGRACEFUL {
                    EVENT_BEHAVIOR_CHANGED_TO_UNGRACEFUL
                } else {
                    EVENT_BEHAVIOR_CHANGED_TO_GRACEFUL
                })
            }
            // The action asks for what the execution has already reached: it is admitted,
            // and there is nothing for it to change.
            ExecutionUpdate::Pause | ExecutionUpdate::Resume | ExecutionUpdate::SwitchMode(_) => {
                None
            }
        };

        if event.is_some() || comment.is_some() {
            if let Some(execution) = self.executions.get_mut(key) {
                execution.updated_at = at;
                if comment.is_some() {
                    execution.comment = comment.clone();
                }
            }
        }
        if let Some(event) = event {
            self.push_detailed_event(key, event, None, comment, None, at);
        }
        ExecutionUpdateOutcome::Applied
    }

    /// Move the step at `index` to `status`, timestamping it, and report its name.
    ///
    /// `startTime` is filled in only when the step carries none: a step the clock stopped
    /// at for an approval already records the instant it was reached.
    fn mark_step(
        &mut self,
        key: &(String, String),
        index: usize,
        status: &str,
        at: DateTime,
        finished: bool,
    ) -> Option<String> {
        let execution = self.executions.get_mut(key)?;
        let step = execution.step_states.get_mut(index)?;
        step.status = Some(status.to_string());
        step.start_time = step.start_time.or(Some(at));
        if finished {
            step.end_time = Some(at);
        }
        step.name.clone()
    }

    /// Record when the clock next owes `key` a transition — `None` for never — keeping
    /// the due-transition index and the execution's own `outcome_due_at` in step, so the
    /// two can never disagree about what is outstanding.
    fn schedule(&mut self, key: &(String, String), due: Option<SystemTime>) {
        let previous = self
            .executions
            .get(key)
            .and_then(|execution| execution.outcome_due_at);
        if let Some(previous) = previous {
            self.due_transitions
                .remove(&(previous, key.0.clone(), key.1.clone()));
        }
        if let Some(execution) = self.executions.get_mut(key) {
            execution.outcome_due_at = due;
        }
        if let Some(due) = due {
            self.due_transitions
                .insert((due, key.0.clone(), key.1.clone()));
        }
    }

    /// The execution has reached a state it never leaves: the clock owes it nothing more,
    /// and it no longer holds its plan — which becomes deletable and executable again.
    fn finish_execution(&mut self, key: &(String, String)) {
        self.schedule(key, None);
        if let Some(plan) = self.plans.get_mut(&key.0) {
            if plan.active_execution.as_ref() == Some(&key.1) {
                plan.active_execution = None;
            }
        }
    }

    /// Append one execution-level event to an execution's timeline, chained to the one
    /// before it.
    pub fn push_event(
        &mut self,
        key: &(String, String),
        event_type: &str,
        error: Option<String>,
        at: DateTime,
    ) {
        self.push_detailed_event(key, event_type, None, None, error, at);
    }

    /// Append one event, which may name the step it is about and carry a description.
    ///
    /// The timeline is bounded by [`MAX_EVENTS_PER_EXECUTION`]: at the ceiling the oldest
    /// event is dropped, and the oldest one kept is unchained, so what a caller can read
    /// is always a complete chain.
    pub fn push_detailed_event(
        &mut self,
        key: &(String, String),
        event_type: &str,
        step_name: Option<String>,
        description: Option<String>,
        error: Option<String>,
        at: DateTime,
    ) {
        let event_id = format!(
            "{:016x}",
            self.next_seed().wrapping_mul(EXECUTION_ID_MULTIPLIER)
        );
        if let Some(execution) = self.executions.get_mut(key) {
            let previous_event_id = execution.events.last().map(|event| event.event_id.clone());
            let event = ExecutionEvent {
                timestamp: Some(at),
                r#type: Some(event_type.to_string()),
                step_name,
                execution_block_type: None,
                resources: None,
                error,
                description,
                event_id,
                previous_event_id,
            };
            execution.events.push(event);
            let overflow = execution
                .events
                .len()
                .saturating_sub(MAX_EVENTS_PER_EXECUTION);
            if overflow > 0 {
                execution.events.drain(..overflow);
                if let Some(oldest) = execution.events.first_mut() {
                    oldest.previous_event_id = None;
                }
            }
        }
    }

    /// Store `execution` against its plan, registering the transition the clock owes it
    /// and opening its event timeline.
    ///
    /// The caller must have made room with [`Self::room_for_execution`] first.
    pub fn insert_execution(&mut self, execution: StoredExecution) {
        let key = (execution.plan_arn.clone(), execution.execution_id.clone());
        let started_at = execution.start_time;
        if let Some(due) = execution.outcome_due_at {
            self.due_transitions
                .insert((due, key.0.clone(), key.1.clone()));
        }
        if let Some(plan) = self.plans.get_mut(&key.0) {
            plan.executions.push(key.1.clone());
            plan.active_execution = Some(key.1.clone());
        }
        self.executions.insert(key.clone(), execution);
        self.push_event(&key, EVENT_EXECUTION_PENDING, None, started_at);
    }

    /// Make room for one more execution of `plan_arn`, and report whether there is any.
    ///
    /// Under [`MAX_EXECUTIONS_PER_PLAN`] there is nothing to do. At it, the oldest
    /// execution that has reached a state it never leaves is dropped; if none has, there
    /// is no room and the caller refuses the request.
    pub fn room_for_execution(&mut self, plan_arn: &str) -> bool {
        // The common path reads the list by reference and allocates nothing; only a plan
        // at the ceiling pays for collecting its candidate ids.
        let at_ceiling = self
            .plans
            .get(plan_arn)
            .is_some_and(|plan| plan.executions.len() >= MAX_EXECUTIONS_PER_PLAN);
        if !at_ceiling {
            return true;
        }
        let recorded: Vec<String> = self
            .plans
            .get(plan_arn)
            .map(|plan| plan.executions.clone())
            .unwrap_or_default();
        let evictable = recorded.into_iter().find(|execution_id| {
            self.executions
                .get(&(plan_arn.to_string(), execution_id.clone()))
                .is_some_and(StoredExecution::is_terminal)
        });
        let Some(evictable) = evictable else {
            return false;
        };
        self.forget_execution(plan_arn, &evictable);
        true
    }

    /// Record `token` as having started `execution_id`, evicting the oldest record once
    /// the cache is at [`MAX_CLIENT_TOKENS`].
    pub fn record_client_token(
        &mut self,
        token: String,
        fingerprint: String,
        plan_arn: String,
        execution_id: String,
    ) {
        while self.client_tokens.len() >= MAX_CLIENT_TOKENS {
            let Some((_, evicted)) = self.client_token_order.pop_first() else {
                break;
            };
            self.client_tokens.remove(&evicted);
        }
        let recorded_at = self.next_seed();
        self.client_token_order.insert(recorded_at, token.clone());
        self.client_tokens.insert(
            token,
            TokenRecord {
                fingerprint,
                plan_arn,
                execution_id,
                recorded_at,
            },
        );
    }

    /// Drop one execution and everything that points at it.
    fn forget_execution(&mut self, plan_arn: &str, execution_id: &str) {
        let key = (plan_arn.to_string(), execution_id.to_string());
        if let Some(execution) = self.executions.remove(&key) {
            if let Some(due) = execution.outcome_due_at {
                self.due_transitions
                    .remove(&(due, key.0.clone(), key.1.clone()));
            }
        }
        if let Some(plan) = self.plans.get_mut(plan_arn) {
            plan.executions.retain(|id| id != execution_id);
            if plan.active_execution.as_deref() == Some(execution_id) {
                plan.active_execution = None;
            }
        }
        self.forget_tokens_for(|record| record.execution_id == execution_id);
    }

    /// Drop a plan and everything recorded against it, so nothing outlives the plan it
    /// belongs to.
    pub fn forget_plan(&mut self, plan_arn: &str) {
        self.plans.remove(plan_arn);
        self.executions.retain(|(arn, _), _| arn != plan_arn);
        self.due_transitions.retain(|(_, arn, _)| arn != plan_arn);
        self.forget_tokens_for(|record| record.plan_arn == plan_arn);
    }

    /// Drop every token record `doomed` selects, from the cache and from its FIFO.
    fn forget_tokens_for(&mut self, doomed: impl Fn(&TokenRecord) -> bool) {
        let mut orders = Vec::new();
        self.client_tokens.retain(|_, record| {
            if doomed(record) {
                orders.push(record.recorded_at);
                false
            } else {
                true
            }
        });
        for order in orders {
            self.client_token_order.remove(&order);
        }
    }
}

/// The base-36 digits a plan ARN's suffix is drawn from: `[a-z0-9]`, as `PlanArn`'s
/// pattern requires.
const BASE36_DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// How many distinct 6-character base-36 suffixes there are.
const PLAN_SUFFIX_SPACE: u64 = 36u64.pow(6);

/// Scrambles the seed so successive plans do not get visibly consecutive suffixes.
/// Odd and not a multiple of three, hence coprime with `36^6 = 2^12 * 3^12`, so
/// `seed -> suffix` is injective and two plans never collide on one suffix.
const PLAN_SUFFIX_MULTIPLIER: u64 = 2_654_435_761;

/// Scrambles the seed into an execution id. Odd, so multiplication modulo 2^64 is a
/// bijection and two executions never share an id.
const EXECUTION_ID_MULTIPLIER: u64 = 0x9E37_79B9_7F4A_7C15;

/// The 6-character suffix a plan ARN ends in, minted from the store's seed.
pub fn plan_arn_suffix(seed: u64) -> String {
    // `(seed % SPACE) * MULTIPLIER` is at most 2176782335 * 2654435761 < u64::MAX, so
    // the product is exact and the reduction below is a true modular multiplication.
    let mut value = (seed % PLAN_SUFFIX_SPACE) * PLAN_SUFFIX_MULTIPLIER % PLAN_SUFFIX_SPACE;
    let mut digits = [b'0'; 6];
    for slot in digits.iter_mut().rev() {
        *slot = BASE36_DIGITS[(value % 36) as usize];
        value /= 36;
    }
    String::from_utf8(digits.to_vec()).expect("base-36 digits are ASCII")
}

/// The ARN for a plan: `arn:aws:arc-region-switch::<account>:plan/<name>:<suffix>`, as
/// `PlanArn`'s pattern requires (the Region segment is empty — the plan is global).
pub fn plan_arn(name: &str, seed: u64) -> String {
    ::engine::emulator_arn(
        "arc-region-switch",
        "",
        &format!("plan/{name}:{}", plan_arn_suffix(seed)),
    )
}

/// The id for an execution: `<region>/<16 hex digits>`, the form `RecoveryExecutionId`'s
/// pattern gives a prior execution's id, minted from the store's seed.
pub fn execution_id(region: &str, seed: u64) -> String {
    format!(
        "{region}/{:016x}",
        seed.wrapping_mul(EXECUTION_ID_MULTIPLIER)
    )
}

/// The two parts of an `ExecutionId`: the Region it ran in and the BARE id after it.
///
/// An `ExecutionId` is `<region>/<id>` ([`execution_id`], the form
/// `RecoveryExecutionId`'s pattern spells), so anything that reports the two separately —
/// the `DeletePlan` refusal that names the execution holding the plan — renders each part
/// once instead of repeating the Region inside the id. A value carrying no separator is
/// all id and names no Region.
pub fn execution_id_parts(execution_id: &str) -> (&str, &str) {
    execution_id.split_once('/').unwrap_or(("", execution_id))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsArcregionswitchArcregionswitchState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsArcregionswitchArcregionswitchState>)
}

/// The Region a request is scoped to: the Region of its SigV4 credential scope, or
/// [`DEFAULT_REGION`] when it carries none.
///
/// This service's plans are global (their ARN has an empty Region segment), so a
/// request's Region matters only to the `...InRegion` operations, which report what the
/// Region they were called in holds.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}
