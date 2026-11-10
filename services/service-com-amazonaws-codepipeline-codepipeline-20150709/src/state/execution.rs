// SPDX-License-Identifier: CC0-1.0

//! The pipeline EXECUTION plane: what `StartPipelineExecution` sets running, how it
//! moves forward, and what it publishes to `GetPipelineExecution`,
//! `ListPipelineExecutions` and `GetPipelineState`.
//!
//! # The rule this module implements
//!
//! `StartPipelineExecution` does not just mint an id: the service then DRIVES the
//! execution. Each stage's actions run in `runOrder` order (an action never starts
//! before every action of a lower run order has finished, and the actions of ONE run
//! order run concurrently: they all start at the same instant and none of them waits for
//! a sibling), an action's artifacts are produced into the
//! pipeline's `artifactStore`, and the execution reaches a terminal
//! `PipelineExecutionStatus` (`Succeeded`, `Failed`, `Stopped`, `Superseded`) that the
//! reporting operations report. So the execution's progress is a function of the stored
//! declaration and the CLOCK, not of what anybody asked for.
//!
//! A pipeline runs one execution PER STAGE, not one execution at a time: an execution is
//! never held out of the pipeline because another is in flight — it runs every stage whose
//! turn is free and waits at the boundary of the stage another execution holds, however
//! long that one stays there (waiting for an approval, for a job worker, or for a disabled
//! transition to be enabled). `PipelineDeclaration$executionMode` decides what happens to
//! the one that is waiting: in `SUPERSEDED` — the default — a later execution that reaches
//! the same boundary supersedes it, `QUEUED` lets them in in the order they arrived, and
//! `PARALLEL` executions wait for no other one at all. [`super::driver`] applies it.
//!
//! # Where the work happens, and why a read may perform it
//!
//! Nothing here sleeps: every step carries the instant it became DUE ([`ACTION_STEP`]
//! after the step before it finished), and [`advance`] materialises only the steps whose
//! instant has already passed. That is exactly the "a reporting operation may write only
//! to materialise a change the clock already made due" case: an action owed at `t` is
//! performed on the first call after `t`, whoever makes it, and a caller that polls
//! faster never makes it run sooner.
//!
//! A client that starts an execution and then watches the artifact store makes no such
//! call at all, so [`super::clock`] waits for those instants on the service's own
//! behalf and materialises the same steps the same way. Which step is due, and when,
//! is decided here either way.
//!
//! # What this engine can and cannot run
//!
//! An action is only reported `Succeeded` when its real work was actually performed:
//!
//! * `Approval` actions need no external work: the action goes `InProgress` with a
//!   token and waits for `PutApprovalResult`.
//! * `Source`/`Deploy` actions whose provider is `S3` move real bytes: the source
//!   action reads the object and writes the artifact into the artifact store, and the
//!   deploy action reads the artifact and writes the delivered object — both through
//!   in-process S3 calls over the framework's callback. Which object the source action
//!   reads is the declaration's, under the key and at the version the start's
//!   `SourceRevisionOverride`s pin for it ([`s3_source_pin`]), and at the object's
//!   current version when nothing pinned one.
//! * a `Source` action whose provider is `CodeCommit` resolves the branch its
//!   configuration names to that branch's tip commit and writes the repository's files
//!   at that commit into the artifact store, through in-process CodeCommit and S3
//!   calls; the commit id is the revision the execution ran and the external execution
//!   the action reports.
//! * an action whose provider is `Lambda` performs its work by INVOKING the function
//!   its configuration names, through an in-process Lambda call, and its outcome is
//!   what that invocation answered.
//! * every other provider (CodeBuild, CloudFormation, a custom job worker …) is work
//!   this emulator cannot perform, so its action FAILS with a reason saying so and the
//!   execution reaches `Failed`. Nothing is ever reported as having run when nothing
//!   ran.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

use super::to_date_time;
use crate::ops::types::{
    ActionDeclaration, ActionExecution, ActionState, ErrorDetails, PipelineDeclaration, StageState,
};

/// How long after the step before it an action's work becomes due.
///
/// A real stage transition is not instantaneous, and a caller acting straight after
/// `StartPipelineExecution` must still observe the transient state (an `InProgress`
/// execution, a stage that has not started, an approval not yet reached) and the state
/// errors that go with it. AWS's own span is neither documented nor constant, so this
/// one is CHOSEN: 300 ms, inside the 250-750 ms band this project fixes for a
/// clock-owed transition. The instant is stored per step and compared against the
/// clock, so nothing sleeps.
pub const ACTION_STEP: Duration = Duration::from_millis(300);

/// How long an in-flight action may stay claimed before another call may run it again.
///
/// An action's work is performed without the store's lock held (it does in-process I/O),
/// so the planner marks it claimed to keep a second concurrent call from running it
/// twice. A request that is dropped mid-flight would otherwise leave the action claimed
/// for ever, so a claim older than this is reclaimed.
pub const CLAIM_TIMEOUT: Duration = Duration::from_secs(5);

/// The most steps one [`advance`] call materialises before it returns.
///
/// Every step either finishes a unit of work or stops the walk, so this only bounds how
/// much one request may do: the next call continues from the stored state.
pub const MAX_ADVANCE_STEPS: usize = 32;

/// The most executions one pipeline may have in flight (started or queued) before
/// `StartPipelineExecution` refuses another with
/// `ConcurrentPipelineExecutionsLimitExceededException`.
///
/// AWS publishes a default quota of 50 concurrent executions per pipeline for the
/// QUEUED and PARALLEL execution modes; the figure is adjustable per account, so it is
/// this emulator's bound rather than a universal constant, and it is what keeps the
/// execution store from growing without limit under a burst of starts.
pub const MAX_CONCURRENT_EXECUTIONS_PER_PIPELINE: usize = 50;

/// `PipelineExecutionStatus` values this engine publishes.
pub const STATUS_SUCCEEDED: &str = "Succeeded";
/// See [`STATUS_SUCCEEDED`].
pub const STATUS_FAILED: &str = "Failed";
/// See [`STATUS_SUCCEEDED`].
pub const STATUS_STOPPED: &str = "Stopped";
/// See [`STATUS_SUCCEEDED`].
pub const STATUS_STOPPING: &str = "Stopping";
/// See [`STATUS_SUCCEEDED`].
pub const STATUS_SUPERSEDED: &str = "Superseded";

/// The most entries one artifact archive may carry when a deploy action extracts it.
///
/// An archive is caller content: its entry COUNT, each entry's decompressed size and
/// their sum all have to be bounded before any of it is allocated, because a deflate
/// stream expands by up to ~1000:1 and the compressed input is bounded only by the S3
/// emulator's per-bucket budget. Past any of these bounds the action fails with its
/// reason instead of allocating. The counts are this emulator's own.
pub const MAX_ARCHIVE_ENTRIES: usize = 1_024;

/// The most bytes ONE extracted entry may carry.
///
/// Equal to [`MAX_EXTRACTED_BYTES`], because a single entry may legitimately be the
/// whole archive's content.
pub const MAX_EXTRACTED_ENTRY_BYTES: usize = MAX_EXTRACTED_BYTES;

/// The most bytes one artifact archive may expand to in total.
///
/// 16 MiB, the same figure as the S3 emulator's `MAX_OBJECT_BYTES_PER_BUCKET`: every
/// extracted entry is written to a bucket, so an extraction larger than that budget
/// could not be delivered anyway, and the expansion ratio over a 16 MiB artifact is
/// therefore capped at 1:1 rather than at deflate's ~1000:1.
pub const MAX_EXTRACTED_BYTES: usize = 16 * 1024 * 1024;

/// The most files one source repository's commit may carry into an artifact.
///
/// A source action packs the files of a commit, which is caller content: the count has
/// to be bounded before any of it is read. The same figure as
/// [`MAX_ARCHIVE_ENTRIES`], because what is packed here is what an extracting deploy
/// action later unpacks. Past it the action fails with its reason instead of reading.
pub const MAX_SOURCE_FILES: usize = MAX_ARCHIVE_ENTRIES;

/// The most bytes one source repository's commit may carry into an artifact.
///
/// The same figure as [`MAX_EXTRACTED_BYTES`], for the same reason: the artifact is
/// written to a bucket, so a commit larger than that budget could not be stored anyway.
pub const MAX_SOURCE_BYTES: usize = MAX_EXTRACTED_BYTES;

/// The `StageTransitionType` whose hold keeps an execution OUT of a stage.
pub const TRANSITION_INBOUND: &str = "Inbound";

/// The `StageTransitionType` whose hold keeps an execution from LEAVING a stage.
pub const TRANSITION_OUTBOUND: &str = "Outbound";

/// The `(stage name, StageTransitionType)` pairs a caller has DISABLED on `pipeline`.
///
/// The transitions a caller changed live on the stored pipeline
/// ([`super::Pipeline::set_transition`]); a transition no call has changed is enabled,
/// which is the state every stage starts in. Taken as an owned set so the driver can
/// consult it while it writes to the executions of that same store.
pub fn disabled_transitions(pipeline: &super::Pipeline) -> BTreeSet<(String, String)> {
    pipeline
        .transitions
        .iter()
        .filter(|(_, transition)| !transition.enabled)
        .map(|((stage, kind), _)| (stage.clone(), kind.clone()))
        .collect()
}

/// Whether the `kind` transition of `stage` is held (disabled).
pub fn transition_held(
    disabled: &BTreeSet<(String, String)>,
    stage: &str,
    kind: &str,
) -> bool {
    disabled.contains(&(stage.to_string(), kind.to_string()))
}

/// The `FailureType` reported for work the action's own run could not complete
/// (`smithy_query query="shape" shape="FailureType"`).
pub const FAILURE_JOB_FAILED: &str = "JobFailed";

/// The `FailureType` reported for a declaration this emulator cannot execute at all.
pub const FAILURE_CONFIGURATION_ERROR: &str = "ConfigurationError";

/// The `FailureType` reported when the REVISION a source action was asked to source
/// cannot be obtained at all: the object, the branch tip or the image the action's
/// configuration names is not there to be read.
///
/// From the same modelled enum, which carries a value for exactly that condition —
/// the revision is unavailable — rather than for work that ran and failed
/// ([`FAILURE_JOB_FAILED`]) or for a declaration that could never run
/// ([`FAILURE_CONFIGURATION_ERROR`]).
pub const FAILURE_REVISION_UNAVAILABLE: &str = "RevisionUnavailable";

/// The `SourceRevisionType` whose value is the S3 object VERSION a source action reads.
pub const SOURCE_REVISION_TYPE_S3_OBJECT_VERSION_ID: &str = "S3_OBJECT_VERSION_ID";

/// The `SourceRevisionType` whose value is the S3 object KEY a source action reads.
pub const SOURCE_REVISION_TYPE_S3_OBJECT_KEY: &str = "S3_OBJECT_KEY";

/// What an execution's source-revision overrides pin for ONE S3 source action.
///
/// Each override carries the `revisionType` that says what kind of revision its value
/// is, so only the two S3 types name anything about an S3 object; "either of the types
/// can be used independently, or they can be used together to override the source with
/// a specific ObjectKey and VersionID".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct S3SourcePin {
    /// The object key an `S3_OBJECT_KEY` override names, when one does.
    pub key: Option<String>,
    /// The object version an `S3_OBJECT_VERSION_ID` override names, when one does.
    pub version_id: Option<String>,
}

/// What `overrides` pin for the S3 source action named `action_name`.
///
/// Only an override of an S3 revision type pins an S3 object: an override naming a
/// commit id or an image digest describes another provider's revision, and a revision
/// merely REPORTED for an action (`PutActionRevision`) is not an override at all, so
/// neither selects an object or a version. An action nothing pins reads the object its
/// configuration declares, at that object's current version.
pub fn s3_source_pin(
    overrides: &[crate::ops::types::SourceRevisionOverride],
    action_name: &str,
) -> S3SourcePin {
    let mut pin = S3SourcePin::default();
    for source_override in overrides
        .iter()
        .filter(|source_override| source_override.action_name == action_name)
    {
        match source_override.revision_type.as_str() {
            SOURCE_REVISION_TYPE_S3_OBJECT_KEY => {
                pin.key = Some(source_override.revision_value.clone());
            }
            SOURCE_REVISION_TYPE_S3_OBJECT_VERSION_ID => {
                pin.version_id = Some(source_override.revision_value.clone());
            }
            _ => {}
        }
    }
    pin
}

/// Where an artifact's bytes live: one object in the pipeline's artifact store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRef {
    /// The artifact store bucket.
    pub bucket: String,
    /// The object key the artifact's bytes were written to.
    pub key: String,
    /// The revision the artifact was produced from, when the producing action has one.
    pub revision: Option<String>,
    /// The customer managed key the object at this location is encrypted under, when
    /// the pipeline's artifact store declares one (`ArtifactStore$encryptionKey`: "the
    /// encryption key used to encrypt the data in the artifact store").
    ///
    /// It travels with the LOCATION because it is how the object there is written and
    /// read: every artifact of one execution lands in that store, so the key the store
    /// declares is the key each of its objects carries.
    pub encryption: Option<String>,
}

/// One action of one stage of one execution, as the engine runs it.
///
/// It carries only what the RUN is: what the action IS — its type, its configuration,
/// the artifacts it consumes and produces — is read from the pipeline declaration this
/// execution pinned ([`super::Pipeline::declaration_at`]), so a declaration is stored
/// once per pipeline version rather than copied into every execution record.
#[derive(Debug, Clone)]
pub struct ActionRun {
    /// `ActionDeclaration$name`.
    pub name: String,
    /// `ActionDeclaration$runOrder`, defaulted to 1 ("The default is 1").
    pub run_order: i32,
    /// The id this action's execution is reported under.
    pub action_execution_id: String,
    /// An `ActionExecutionStatus` value, or `None` while the action has not started.
    pub status: Option<String>,
    /// When this action's work becomes due, or `None` while it is not scheduled.
    pub due: Option<SystemTime>,
    /// Set while a call is performing this action's work (see [`CLAIM_TIMEOUT`]).
    pub claimed: Option<SystemTime>,
    /// The approval token an `Approval` action waits on.
    pub token: Option<String>,
    /// A human-readable summary of what the action did.
    pub summary: Option<String>,
    /// Who last moved the action (the approver, for an approval).
    pub last_updated_by: Option<String>,
    /// Why the action failed, when it did.
    pub error: Option<(String, String)>,
    /// When the action's status last changed.
    pub last_status_change: SystemTime,
    /// When the execution STARTED this action, or `None` while it has not reached it.
    pub start_time: Option<SystemTime>,
    /// The id of the run in the external system that performed this action's work,
    /// when the work was performed by one: the commit a source action resolved, the
    /// function an invoke action ran. `None` while nothing external has run.
    pub external_execution_id: Option<String>,
    /// The run this action is WAITING FOR in the service that performs its work, when
    /// that work is a job of another service rather than something one call does: the
    /// build a `CodeBuild` action started. `None` for every action whose work finishes
    /// inside the call that started it.
    ///
    /// It is what lets the engine come BACK to a started run: an action holding one stays
    /// `InProgress` and is settled from that run's own terminal status, which is read each
    /// time the action's next step falls due ([`ACTION_STEP`]) rather than waited for.
    pub external_run: Option<String>,
}

impl ActionRun {
    /// This action as `GetPipelineState` reports it.
    pub fn to_state(&self) -> ActionState {
        ActionState {
            action_name: Some(self.name.clone()),
            // A revision is recorded by `PutActionRevision`, which this slice does not
            // serve, so none is reported rather than invented.
            current_revision: None,
            latest_execution: self.status.as_ref().map(|status| ActionExecution {
                action_execution_id: Some(self.action_execution_id.clone()),
                status: Some(status.clone()),
                summary: self.summary.clone(),
                last_status_change: Some(to_date_time(self.last_status_change)),
                token: self.token.clone(),
                last_updated_by: self.last_updated_by.clone(),
                external_execution_id: self.external_execution_id.clone(),
                external_execution_url: None,
                percent_complete: None,
                error_details: self.error.as_ref().map(|(code, message)| ErrorDetails {
                    code: Some(code.clone()),
                    message: Some(message.clone()),
                }),
                log_stream_arn: None,
            }),
            entity_url: None,
            revision_url: None,
        }
    }

    /// True once this action can no longer change on its own.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_deref(),
            Some("Succeeded" | "Failed" | "Abandoned")
        )
    }

    /// True while this action is running or waiting for an approval.
    pub fn is_in_progress(&self) -> bool {
        self.status.as_deref() == Some("InProgress")
    }
}

/// One stage of one execution, as the engine runs it.
#[derive(Debug, Clone)]
pub struct StageRun {
    /// `StageDeclaration$name`.
    pub name: String,
    /// WHICH declared stage of the pinned declaration this is the run of.
    ///
    /// A plan does not have to hold every declared stage, nor hold them from the
    /// pipeline's first one: a stage rollback runs ONE stage, so what each action of a
    /// plan IS is read from the declared stage this names rather than from the run's
    /// position in the plan ([`declared_action`]).
    pub declaration_index: usize,
    /// A `StageExecutionStatus` value, or `None` while the stage has not been entered.
    pub status: Option<String>,
    /// When this execution's ENTRY into the stage becomes due.
    ///
    /// Entering a stage is a clock-owed transition, a span long like every other step
    /// ([`ACTION_STEP`]), so a plan records the instant each of its stages may be entered
    /// ([`plan_stage_at`]) and the execution is at that boundary until it falls due. Every
    /// later hold at the same boundary — the turn of a stage another execution holds, a
    /// disabled transition, a gate a caller overrode — records a fresh instant the same
    /// way, so the stage keeps reporting the run that just left it until the next
    /// execution really enters. `None` would enter as soon as its turn comes, which is
    /// what a plan built before this was recorded means.
    pub entry_due: Option<SystemTime>,
    /// This stage's actions, in declaration order.
    pub actions: Vec<ActionRun>,
    /// The condition site whose MET condition decided this execution's run of the stage,
    /// as a `ConditionType` value, or `None` while no condition has been met here.
    ///
    /// A condition is MET when a rule of it reached a FAILING verdict, and the `Result`
    /// it declares is then what the execution does ([`super::rules`]). The site that
    /// decided it is recorded because the gate is what `OverrideStageCondition` acts on:
    /// a condition nothing was met at is not in a state that can be overridden, and
    /// `GetPipelineState` reports the run of each declared site from the same fact.
    pub blocked_by: Option<String>,
    /// The condition sites a caller has OVERRIDDEN for this run of the stage, as
    /// `ConditionType` values.
    ///
    /// `OverrideStageCondition` is "used to override a stage condition", so an overridden
    /// site is not evaluated again for this run — the execution carries on past the gate
    /// — and `GetPipelineState` reports its run as `Overridden`.
    pub overridden: BTreeSet<String>,
}

impl StageRun {
    /// True once this stage can no longer change on its own.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_deref(),
            Some("Succeeded" | "Failed" | "Stopped" | "Skipped" | "Cancelled")
        )
    }

    /// True while this stage has been entered and has not reached an outcome.
    pub fn is_in_progress(&self) -> bool {
        self.status.is_some() && !self.is_terminal()
    }
}

/// The declared stage `run` is the execution's run of, from the declaration the
/// execution pinned.
pub fn declared_stage<'a>(
    declaration: &'a PipelineDeclaration,
    run: &StageRun,
) -> Option<&'a crate::ops::types::StageDeclaration> {
    declaration.stages.get(run.declaration_index)
}

/// The declared action the `action`th action of `run` runs.
///
/// What an action IS — its type, its configuration, the artifacts it consumes and
/// produces — is read HERE, from the stage of the pinned declaration the run names, so
/// a plan that holds only some of the declaration's stages still reads the right
/// action. `None` means the pinned declaration no longer declares it at all.
pub fn declared_action<'a>(
    declaration: &'a PipelineDeclaration,
    run: &StageRun,
    action: usize,
) -> Option<&'a ActionDeclaration> {
    declared_stage(declaration, run)?.actions.get(action)
}

/// A `StopPipelineExecution` request recorded against an execution.
#[derive(Debug, Clone)]
pub struct StopRequest {
    /// `StopPipelineExecutionInput$abandon`: stop the execution without waiting for the
    /// in-flight actions to finish.
    pub abandon: bool,
    /// The caller's reason, when it gave one.
    pub reason: Option<String>,
    /// When the stop was requested.
    pub requested_at: SystemTime,
}

/// The stage/action plan an execution runs: taken from the pipeline declaration at the
/// instant the execution starts, so a later `UpdatePipeline` does not rewrite a running
/// execution.
pub fn plan_stages(declaration: &PipelineDeclaration, now: SystemTime) -> Vec<StageRun> {
    (0..declaration.stages.len())
        .filter_map(|index| plan_stage_at(declaration, index, now))
        .collect()
}

/// The plan a rollback of ONE stage runs: that declared stage alone, nothing started.
///
/// `None` when the declaration declares no stage of that name, which is what a stage
/// rollback refuses as an unknown stage.
pub fn plan_stage(
    declaration: &PipelineDeclaration,
    stage_name: &str,
    now: SystemTime,
) -> Option<StageRun> {
    let index = declaration
        .stages
        .iter()
        .position(|stage| stage.name == stage_name)?;
    plan_stage_at(declaration, index, now)
}

/// One declared stage, as the engine will run it.
///
/// The stage is not entered at the instant the plan is made: ENTERING a stage is itself a
/// clock-owed transition, due a span ([`ACTION_STEP`]) after the call that started the
/// execution, exactly as being let in past a boundary that held the execution is. So a
/// caller acting straight after `StartPipelineExecution` observes the execution
/// `InProgress` with no stage entered — which is the state a newer execution SUPERSEDES
/// in `SUPERSEDED` mode, because an execution that has not entered a stage is still
/// waiting at that boundary. For the stages after the first the instant is long past by
/// the time the execution reaches them (each earlier step took at least that span), so
/// this holds nothing up twice.
fn plan_stage_at(
    declaration: &PipelineDeclaration,
    index: usize,
    now: SystemTime,
) -> Option<StageRun> {
    let stage = declaration.stages.get(index)?;
    Some(StageRun {
        name: stage.name.clone(),
        declaration_index: index,
        status: None,
        entry_due: Some(now + ACTION_STEP),
        blocked_by: None,
        overridden: BTreeSet::new(),
        actions: stage
            .actions
            .iter()
            .map(|action| plan_action(action, now))
            .collect(),
    })
}

/// One declared action, as the engine will run it.
fn plan_action(action: &ActionDeclaration, now: SystemTime) -> ActionRun {
    ActionRun {
        name: action.name.clone(),
        // `ActionDeclaration$runOrder` documents "The default is 1".
        run_order: action.run_order.unwrap_or(1),
        action_execution_id: super::random_uuid_v4(),
        status: None,
        due: None,
        claimed: None,
        token: None,
        summary: None,
        last_updated_by: None,
        error: None,
        last_status_change: now,
        start_time: None,
        external_execution_id: None,
        external_run: None,
    }
}

/// The artifact-store bucket a declaration's executions write their artifacts to, with
/// the customer managed key that store declares, if any.
///
/// `CreatePipeline` documents "you must include either artifactStore or artifactStores",
/// so exactly one of the two is present on a stored declaration; with `artifactStores`
/// the entries are per Region and the one named for `region` is this execution's.
pub fn artifact_store(
    declaration: &PipelineDeclaration,
    region: &str,
) -> Option<(String, Option<String>)> {
    let store = match &declaration.artifact_store {
        Some(store) => store,
        None => {
            let stores = declaration.artifact_stores.as_ref()?;
            stores.get(region).or_else(|| stores.values().next())?
        }
    };
    Some((
        store.location.clone(),
        store.encryption_key.as_ref().map(|key| key.id.clone()),
    ))
}

/// `#{namespace.key}` placeholders resolved against the variables an execution has
/// already produced, up to [`super::MAX_RESOLVED_CONFIGURATION_VALUE_BYTES`].
///
/// CodePipeline's variable syntax is `#{<namespace>.<key>}`, with the pipeline-level
/// variables under the `variables` namespace and an action's output variables under the
/// namespace that action declares. A placeholder naming a variable the execution does
/// not carry is left exactly as written, so the action sees (and can fail on) the
/// unresolved reference rather than an empty string.
///
/// The resolved text is accumulated against the SAME byte budget the stored-record
/// reader applies ([`super::MAX_RESOLVED_CONFIGURATION_VALUE_BYTES`]), checked as the
/// text is built: a value whose resolution would pass it is answered UNRESOLVED (the
/// stored text, verbatim) rather than allocated, so the allocation one request can drive
/// is bounded by a stated figure however many placeholders a configuration value carries
/// and however long a reported variable is. A replacement is never re-scanned either, so
/// no chain of references can amplify one request.
pub fn resolve_placeholders(value: &str, variables: &BTreeMap<String, String>) -> String {
    let budget = super::MAX_RESOLVED_CONFIGURATION_VALUE_BYTES;
    let mut resolved = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("#{") {
        let before = &rest[..start];
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            // An unterminated `#{`: the rest of the value is literal text.
            break;
        };
        let name = &after[..end];
        let substitution = variables.get(name).map(String::as_str);
        // A placeholder the execution has no value for keeps its own text, which is what
        // the budget then has to account for.
        let substituted = substitution.map_or(name.len() + 3, str::len);
        if resolved.len() + before.len() + substituted > budget {
            return value.to_string();
        }
        resolved.push_str(before);
        match substitution {
            Some(replacement) => resolved.push_str(replacement),
            None => {
                resolved.push_str("#{");
                resolved.push_str(name);
                resolved.push('}');
            }
        }
        rest = &after[end + 1..];
    }
    if resolved.len() + rest.len() > budget {
        return value.to_string();
    }
    resolved.push_str(rest);
    resolved
}

/// One declared action's configuration as an execution RESOLVED it: every
/// `#{namespace.key}` placeholder replaced by the value that execution carries for it.
///
/// One helper for both readers of a configuration — the engine that runs the action
/// ([`super::driver`]) and the job worker that is served it ([`super::jobs`]) — so the
/// configuration the worker reads and the one the engine runs with cannot drift.
pub fn resolved_configuration(
    action: &ActionDeclaration,
    variables: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    action
        .configuration
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|(key, value)| (key, resolve_placeholders(&value, variables)))
        .collect()
}

/// The `StageState` list `GetPipelineState` reports for one pipeline.
///
/// Each stage reports its declared actions, enriched with the action execution of the
/// newest execution that entered the stage; the inbound transition state is the one the
/// transition calls persisted on the pipeline
/// ([`super::Pipeline::transition_state`]) — and, as the stored-pipeline reader
/// already words it, an inbound transition is the edge INTO a stage, so the first stage
/// reports none unless a caller changed one for it.
///
/// `rules` are the rule executions the condition plane recorded for this pipeline, which
/// is where the run of each declared condition site comes from
/// ([`super::condition_state`]).
pub fn stage_states(
    pipeline: &super::Pipeline,
    executions: &[&super::PipelineExecutionRecord],
    revisions: &BTreeMap<(String, String, String), crate::ops::types::ActionRevision>,
    rules: &[super::RuleExecutionRecord],
) -> Vec<StageState> {
    // One pass over the executions, newest first, builds both indexes: the newest
    // execution that ENTERED each stage, and the newest non-terminal one still waiting
    // to enter it. Without it this walk would rescan every execution for every declared
    // stage. An execution WAITS at exactly one stage — the first it has not entered —
    // so a stage further down the pipeline reports no inbound execution for it.
    let mut entered: HashMap<&str, (&super::PipelineExecutionRecord, &StageRun)> = HashMap::new();
    let mut waiting: HashMap<&str, &super::PipelineExecutionRecord> = HashMap::new();
    for record in executions {
        let mut waits_at: Option<&str> = None;
        for run in &record.stages {
            if run.status.is_some() {
                entered.entry(run.name.as_str()).or_insert((record, run));
            } else if waits_at.is_none() {
                waits_at = Some(run.name.as_str());
            }
        }
        if let (false, Some(stage)) = (is_terminal_status(&record.status), waits_at) {
            waiting.entry(stage).or_insert(record);
        }
    }

    pipeline
        .declaration
        .stages
        .iter()
        .map(|stage| {
            let run = entered.get(stage.name.as_str()).copied();
            let inbound = waiting.get(stage.name.as_str()).copied();
            // Only the INBOUND transition is what this member reports
            // (`StageState$inboundTransitionState`); the outbound one governs leaving
            // the stage and the model binds no member to it here. The rule for what a
            // stage reports lives on the stored pipeline, so the two views of a
            // stage cannot disagree.
            let inbound_transition = pipeline.inbound_transition_state(&stage.name);
            StageState {
                stage_name: Some(stage.name.clone()),
                inbound_execution: inbound.map(|record| crate::ops::types::StageExecution {
                    pipeline_execution_id: record.pipeline_execution_id.clone(),
                    status: record.status.clone(),
                    r#type: Some(record.execution_type.clone()),
                }),
                inbound_executions: None,
                inbound_transition_state: Some(inbound_transition),
                // Every declared action is reported, whether or not an execution has
                // reached it: the one that has run carries its action execution, and the
                // one that has not carries only its name, because there is no outcome
                // for it yet. `currentRevision` is the revision `PutActionRevision`
                // recorded for the action, keyed by `(pipeline, stage, action)`.
                action_states: Some(
                    stage
                        .actions
                        .iter()
                        .map(|action| {
                            let current_revision = revisions
                                .get(&(
                                    pipeline.declaration.name.clone(),
                                    stage.name.clone(),
                                    action.name.clone(),
                                ))
                                .cloned();
                            run.and_then(|(_, run)| {
                                run.actions
                                    .iter()
                                    .find(|candidate| candidate.name == action.name)
                                    .map(ActionRun::to_state)
                            })
                            .map(|state| ActionState {
                                current_revision: current_revision.clone(),
                                ..state
                            })
                            .unwrap_or_else(|| ActionState {
                                action_name: Some(action.name.clone()),
                                current_revision,
                                latest_execution: None,
                                entity_url: None,
                                revision_url: None,
                            })
                        })
                        .collect(),
                ),
                latest_execution: run.map(|(record, run)| crate::ops::types::StageExecution {
                    pipeline_execution_id: record.pipeline_execution_id.clone(),
                    status: run.status.clone().unwrap_or_else(|| record.status.clone()),
                    r#type: Some(record.execution_type.clone()),
                }),
                // A condition set has a state exactly when the stage DECLARES it:
                // `beforeEntry` and `onSuccess` carry their conditions directly, while
                // `onFailure` also carries the stage's failure `result` and retry
                // configuration, which are declaration rather than state. What each
                // site's run WAS is the rule executions the condition plane recorded for
                // the execution that reached the stage.
                before_entry_condition_state: stage.before_entry.as_ref().map(|before_entry| {
                    super::condition_state(
                        &before_entry.conditions,
                        Some(super::rules::CONDITION_TYPE_BEFORE_ENTRY),
                        run.map(|(record, _)| record.pipeline_execution_id.as_str()),
                        run.is_some_and(|(_, run)| {
                            run.overridden
                                .contains(super::rules::CONDITION_TYPE_BEFORE_ENTRY)
                        }),
                        rules,
                    )
                }),
                on_success_condition_state: stage.on_success.as_ref().map(|on_success| {
                    super::condition_state(
                        &on_success.conditions,
                        Some(super::rules::CONDITION_TYPE_ON_SUCCESS),
                        run.map(|(record, _)| record.pipeline_execution_id.as_str()),
                        run.is_some_and(|(_, run)| {
                            run.overridden
                                .contains(super::rules::CONDITION_TYPE_ON_SUCCESS)
                        }),
                        rules,
                    )
                }),
                // `onFailure` is the remediation site: `ConditionType` declares no value
                // for it and this engine evaluates nothing there ([`super::rules`]), so
                // its conditions carry no run.
                on_failure_condition_state: stage
                    .on_failure
                    .as_ref()
                    .and_then(|on_failure| on_failure.conditions.as_deref())
                    .map(|conditions| super::condition_state(conditions, None, None, false, rules)),
                retry_stage_metadata: run.and_then(|(record, _)| {
                    (record.manual_retry_attempts > 0).then(|| {
                        crate::ops::types::RetryStageMetadata {
                            auto_stage_retry_attempt: None,
                            manual_stage_retry_attempt: Some(record.manual_retry_attempts),
                            latest_retry_trigger: Some("ManualStageRetry".to_string()),
                        }
                    })
                }),
            }
        })
        .collect()
}

/// True for a `PipelineExecutionStatus` an execution can no longer leave.
pub fn is_terminal_status(status: &str) -> bool {
    matches!(
        status,
        "Succeeded" | "Failed" | "Stopped" | "Superseded" | "Cancelled"
    )
}

/// The environment variables a CodeBuild action overrides the project's with, as
/// `(name, value, type)` — the three members of the `EnvironmentVariable` that call takes,
/// with the type left to the service's own default where the declaration states none.
pub type EnvironmentOverrides = Vec<(String, String, Option<String>)>;

/// One unit of real work the engine has claimed and must now perform.
#[derive(Debug, Clone)]
pub enum Job {
    /// An S3 source action: read the source object, write it into the artifact store.
    S3Source {
        /// The execution the action belongs to.
        execution_id: String,
        /// The stage's index in the execution's plan.
        stage: usize,
        /// The action's index in that stage.
        action: usize,
        /// The bucket the source object lives in.
        bucket: String,
        /// The source object's key.
        key: String,
        /// The object version the execution pinned, when it pinned one.
        version_id: Option<String>,
        /// Where the produced artifacts are written.
        outputs: Vec<ArtifactRef>,
    },
    /// A CodeCommit source action: resolve the branch's tip commit and write the
    /// repository's files at that commit into the artifact store.
    CodeCommitSource {
        /// The execution the action belongs to.
        execution_id: String,
        /// The stage's index in the execution's plan.
        stage: usize,
        /// The action's index in that stage.
        action: usize,
        /// The repository the action's configuration names.
        repository: String,
        /// The branch whose tip the action reads.
        branch: String,
        /// Where the produced artifacts are written.
        outputs: Vec<ArtifactRef>,
    },
    /// A Lambda action: invoke the function the action's configuration names, passing
    /// it the job event CodePipeline publishes for such an action.
    LambdaInvoke {
        /// The execution the action belongs to.
        execution_id: String,
        /// The stage's index in the execution's plan.
        stage: usize,
        /// The action's index in that stage.
        action: usize,
        /// The function the action invokes.
        function_name: String,
        /// `UserParameters`, the configuration value the event carries verbatim.
        user_parameters: Option<String>,
        /// The id the job event publishes for this action's run.
        job_id: String,
        /// The artifacts the function is handed, by declared name.
        inputs: Vec<(String, ArtifactRef)>,
        /// The artifact locations the function is to write, by declared name.
        outputs: Vec<(String, ArtifactRef)>,
    },
    /// An S3 deploy action: read the input artifact, write the delivered object(s).
    S3Deploy {
        /// The execution the action belongs to.
        execution_id: String,
        /// The stage's index in the execution's plan.
        stage: usize,
        /// The action's index in that stage.
        action: usize,
        /// The artifact being deployed.
        input: ArtifactRef,
        /// The bucket the object is delivered to.
        bucket: String,
        /// The key the archive is delivered under, or — with `extract` — the key prefix
        /// its entries are delivered beneath, which the declaration may leave out so
        /// the entries land at the root of the bucket.
        key: Option<String>,
        /// "Extract file before deploy": every entry of the artifact archive is
        /// delivered under `key` instead of the archive itself.
        extract: bool,
        /// The customer managed key the delivered object is encrypted under, when one
        /// applies: the key the action's own `KMSEncryptionKeyARN` names, and otherwise
        /// the one the pipeline's artifact store declares.
        encryption_key: Option<String>,
    },
    /// An Amazon ECR source action: resolve the image the configuration names and write
    /// the image detail of it into the artifact store.
    EcrSource {
        /// The execution the action belongs to.
        execution_id: String,
        /// The stage's index in the execution's plan.
        stage: usize,
        /// The action's index in that stage.
        action: usize,
        /// The repository the action's configuration names.
        repository: String,
        /// The tag whose image the action sources.
        image_tag: String,
        /// The image the execution was STARTED for, when its start resolved one: the
        /// action sources THAT image rather than whatever the tag names when it runs.
        image_digest: Option<String>,
        /// Where the produced artifacts are written.
        outputs: Vec<ArtifactRef>,
    },
    /// A CodeBuild action: start a build of the project the configuration names.
    CodeBuildStart {
        /// The execution the action belongs to.
        execution_id: String,
        /// The stage's index in the execution's plan.
        stage: usize,
        /// The action's index in that stage.
        action: usize,
        /// The project the action builds.
        project_name: String,
        /// The environment variables the action's configuration overrides the project's
        /// with ([`EnvironmentOverrides`]).
        environment_overrides: EnvironmentOverrides,
    },
    /// The same action's next step: read the build it started, and settle the action
    /// once that build has reached a terminal status.
    CodeBuildPoll {
        /// The execution the action belongs to.
        execution_id: String,
        /// The stage's index in the execution's plan.
        stage: usize,
        /// The action's index in that stage.
        action: usize,
        /// The build the action is waiting for.
        build_id: String,
    },
}

impl Job {
    /// The customer managed key the objects this job WRITES are encrypted under, when
    /// one applies.
    ///
    /// Every object a job writes into the pipeline's artifact store carries the key that
    /// store declares ([`ArtifactRef::encryption`]), and a deploy action's delivered
    /// object carries the one the deploy itself resolved; a job that writes no object has
    /// none. One answer per job, so the key the write uses and the key whose use is
    /// checked with KMS first cannot drift.
    pub fn artifact_encryption(&self) -> Option<&str> {
        match self {
            Self::S3Source { outputs, .. }
            | Self::CodeCommitSource { outputs, .. }
            | Self::EcrSource { outputs, .. } => outputs
                .iter()
                .find_map(|output| output.encryption.as_deref()),
            Self::LambdaInvoke { outputs, .. } => outputs
                .iter()
                .find_map(|(_, output)| output.encryption.as_deref()),
            Self::S3Deploy { encryption_key, .. } => encryption_key.as_deref(),
            Self::CodeBuildStart { .. } | Self::CodeBuildPoll { .. } => None,
        }
    }

    /// Where in the store this job's action lives.
    pub fn target(&self) -> (&str, usize, usize) {
        match self {
            Self::S3Source {
                execution_id,
                stage,
                action,
                ..
            }
            | Self::CodeCommitSource {
                execution_id,
                stage,
                action,
                ..
            }
            | Self::LambdaInvoke {
                execution_id,
                stage,
                action,
                ..
            }
            | Self::S3Deploy {
                execution_id,
                stage,
                action,
                ..
            }
            | Self::EcrSource {
                execution_id,
                stage,
                action,
                ..
            }
            | Self::CodeBuildStart {
                execution_id,
                stage,
                action,
                ..
            }
            | Self::CodeBuildPoll {
                execution_id,
                stage,
                action,
                ..
            } => (execution_id, *stage, *action),
        }
    }
}

/// What performing a [`Job`] produced.
#[derive(Debug, Clone)]
pub enum JobOutcome {
    /// The work was performed: the summary, and the output variables it published.
    Done {
        /// What the action did, as the action's summary.
        summary: String,
        /// The revision the artifacts were produced from, when there is one.
        revision: Option<String>,
        /// The action's output variables, keyed by bare name.
        variables: BTreeMap<String, String>,
        /// The id of the run in the external system that performed the work, when the
        /// work ran in one.
        external_id: Option<String>,
    },
    /// The work could not be performed: a `FailureType` and why.
    Failed {
        /// A `FailureType` value.
        code: String,
        /// Why the action failed.
        message: String,
        /// The id of the external run the work was attempted as, when it reached one.
        external_id: Option<String>,
    },
    /// The work was STARTED in another service and has not finished: the action stays
    /// `InProgress`, and the run it is waiting for is read again when its next step falls
    /// due ([`ACTION_STEP`]).
    ///
    /// Nothing about the action is claimed as done here — no artifact, no revision and no
    /// status but the one it already reports — so an action in this state is one a client
    /// sees running, exactly as it does while a custom action's job waits for its worker.
    Running {
        /// The run in the other service this action is now waiting for, which is both the
        /// handle the next step reads it by and the external execution a client sees.
        external_run: String,
        /// What the action is doing, as the action's summary.
        summary: String,
    },
}
