// SPDX-License-Identifier: CC0-1.0

//! The pipeline, portal, project, access-policy, search, task, action and
//! execution records of [`super::ComAmazonawsIotsitewiseAwsiotsitewiseState`].

use super::*;

/// A stored pipeline: its identity within a workspace, the version a read answers
/// from, every version a later write superseded, and the `tags` the tagging
/// operations read.
#[derive(Debug, Clone)]
pub struct PipelineRecord {
    /// The workspace this pipeline lives in. A pipeline name is "unique within
    /// the workspace", so the pair is the pipeline's identity.
    pub workspace_name: String,
    pub name: String,
    pub arn: String,
    /// The newest version: what a read naming no version answers from.
    pub current: PipelineVersion,
    /// The versions a later write replaced, ascending by version number.
    pub superseded: Vec<PipelineVersion>,
    pub created_at: SystemTime,
    /// The instant the transition the clock owes — from [`Self::settling_state`]
    /// to `ACTIVE` — falls due.
    pub active_at: SystemTime,
    /// The `ResourceState` this pipeline reports until [`Self::active_at`] falls
    /// due: [`STATE_CREATING`] for a freshly created pipeline, [`STATE_UPDATING`]
    /// while a mutating call it already accepted settles.
    pub settling_state: String,
    pub tags: HashMap<String, String>,
}

impl PipelineRecord {
    /// The pipeline's `version` as the modelled `Version` string.
    pub fn version_label(&self) -> String {
        self.current.version.to_string()
    }

    /// The version a `Version` label names — the current one or any it
    /// superseded — or `None` when no write ever produced it.
    ///
    /// Compared NUMERICALLY, after parsing the label the modelled `Version`
    /// pattern (`^(0|([1-9]{1}\d*))$`, at most 10 digits) admits.
    pub fn version(&self, label: &str) -> Option<&PipelineVersion> {
        let wanted: u64 = label.parse().ok()?;
        if self.current.version == wanted {
            return Some(&self.current);
        }
        self.superseded
            .iter()
            .find(|version| version.version == wanted)
    }

    /// When the pipeline was last written: the instant its current version was
    /// recorded.
    pub fn updated_at(&self) -> SystemTime {
        self.current.recorded_at
    }

    /// The pipeline's `ResourceState` as of `now`: its [`Self::settling_state`]
    /// until the owed transition falls due, `ACTIVE` from then on. Derived, so a
    /// read never has to write.
    pub fn state(&self, now: SystemTime) -> &str {
        state_at(&self.settling_state, self.active_at, now)
    }

    /// The pipeline's status as of `now`. No write has failed, so it carries no
    /// error.
    pub fn status(&self, now: SystemTime) -> ResourceStatus {
        ResourceStatus {
            state: Some(self.state(now).to_string()),
            error: None,
        }
    }

    /// Record that an accepted update landed at `now`: the configuration it
    /// carries becomes a NEW version, the version it replaces is kept for a read
    /// that names it, and the pipeline reports `UPDATING` until the transition
    /// this call owes falls due.
    ///
    /// The due instant is STORED, never slept on, so the caller's own call
    /// returns immediately and the transient state is still what a read taken
    /// straight afterwards observes.
    pub fn record_update(
        &mut self,
        description: Option<String>,
        environment_variables: Option<HashMap<String, String>>,
        computations: Vec<ComputeNode>,
        now: SystemTime,
    ) {
        let next = PipelineVersion {
            version: self.current.version + 1,
            description,
            environment_variables,
            computations,
            recorded_at: now,
        };
        let previous = ::std::mem::replace(&mut self.current, next);
        self.superseded.push(previous);
        // The retained history is BOUNDED: past [`MAX_PIPELINE_VERSIONS`] the
        // oldest superseded version is evicted, the way a time series evicts its
        // oldest sample, so no number of updates grows one record without limit.
        let retained = self.superseded.len() + 1;
        if retained > MAX_PIPELINE_VERSIONS {
            self.superseded.drain(..retained - MAX_PIPELINE_VERSIONS);
        }
        self.settling_state = STATE_UPDATING.to_string();
        self.active_at = now + ACTIVATION_DELAY;
    }
}

/// The key a pipeline is stored under in
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::pipelines`]: the workspace it
/// lives in and its own name, since the name is unique only within the workspace.
pub fn pipeline_key(workspace_name: &str, pipeline_name: &str) -> (String, String) {
    (workspace_name.to_string(), pipeline_name.to_string())
}

/// The key an interface relationship is stored under in
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::interface_relationships`]: the
/// asset model the interface is applied to, then the interface asset model.
/// Both are STORED ids, not labels.
pub fn interface_relationship_key(
    asset_model_id: &str,
    interface_asset_model_id: &str,
) -> (String, String) {
    (
        asset_model_id.to_string(),
        interface_asset_model_id.to_string(),
    )
}

/// The ceiling on how many interface relationships this store holds.
///
/// The same reasoning as [`MAX_ASSET_MODELS`]: applying an interface is the only
/// thing that adds one and nothing evicts one, so the map needs a maximum entry
/// count. The emulator's own bound, not a published AWS quota; the operation
/// that would exceed it is refused with the modelled `LimitExceededException`.
pub const MAX_INTERFACE_RELATIONSHIPS: usize = 1_000;

/// The `AssetModelType` value an asset model must carry to be applied to another
/// asset model as an interface: the `AssetModelType` enum's third value, which
/// `CreateAssetModel` documents as "INTERFACE - An interface is a type of model
/// that defines a standard structure".
pub const MODEL_TYPE_INTERFACE: &str = "INTERFACE";

/// The canonical ARN of a pipeline in `region`.
///
/// The resource section carries the WORKSPACE as well as the pipeline name: the
/// name is unique only within its workspace, so two workspaces may each hold a
/// pipeline of one name and the ARN has to tell them apart.
pub fn pipeline_arn(region: &str, workspace_name: &str, pipeline_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{PIPELINE_RESOURCE_TYPE}/{workspace_name}/{pipeline_name}"),
    )
}

/// The `SearchStatus` a search reports before it has begun running — the status
/// `StartSearch` answers with, as the operation documents it ("the response
/// returns immediately with a searchId and an initial status of QUEUED").
pub const SEARCH_QUEUED: &str = "QUEUED";
/// The `SearchStatus` a search reports while it runs.
pub const SEARCH_RUNNING: &str = "RUNNING";
/// The terminal `SearchStatus` of a search whose run finished, and the one
/// `GetSearchResults` is documented to need ("retrieve the results once the
/// search reaches SUCCEEDED").
pub const SEARCH_SUCCEEDED: &str = "SUCCEEDED";

/// The `searchType` a `StartSearch` call that named none runs under, as the
/// `SearchType` shape documents it: "When `searchType` is omitted on a request,
/// the search defaults to QUICK".
pub const DEFAULT_SEARCH_TYPE: &str = "QUICK";

/// How long after the `StartSearch` call the queued-to-running transition it
/// owes falls due.
///
/// CHOSEN, not documented: AWS publishes no duration for a search's lifecycle,
/// only that the call answers immediately with `QUEUED` and that the search
/// runs "in the background". Deliberately non-zero, and towards the long end of
/// the span this emulator allows itself, so a caller that polls straight after
/// the call reliably observes the `QUEUED` the real service reports.
pub const SEARCH_QUEUED_FOR: Duration = Duration::from_millis(600);

/// How long a running search takes to reach [`SEARCH_SUCCEEDED`], counted from
/// the instant it STARTED RUNNING — the transition this one owes is the one the
/// run itself starts, not the one the call starts.
///
/// CHOSEN for the same reason as [`SEARCH_QUEUED_FOR`], and non-zero for the
/// same reason: `RUNNING` is a status `DescribeSearch` is documented to report,
/// so a search has to be observable in it for long enough to be polled.
pub const SEARCH_RUNS_FOR: Duration = Duration::from_millis(600);

/// How many searches one workspace holds.
///
/// The same kind of bound, and for the same reason, as
/// [`MAX_PIPELINES_PER_WORKSPACE`]: `StartSearch` is the only thing that adds a
/// search and nothing evicts one, so the map needs a maximum entry count. The
/// emulator's own bound, not a published AWS quota, and high enough that no
/// ordinary workflow meets it; `StartSearch` refuses past it with the modelled
/// `LimitExceededException` (HTTP 410).
pub const MAX_SEARCHES_PER_WORKSPACE: usize = 100;

/// How many searches the store holds in total, across every workspace.
///
/// The per-workspace cap cannot bound the store on its own: the store holds up
/// to [`MAX_WORKSPACES`] workspaces and each of them its own searches, so the
/// whole map needs an entry count of its own, refused past the cap with the
/// modelled `LimitExceededException` (HTTP 410).
pub const MAX_SEARCHES: usize = 1_000;

/// A search `StartSearch` started: the request it was started from and the
/// instants its lifecycle transitions fall due.
///
/// The status is DERIVED from those instants ([`Self::status`]) rather than
/// stored, so polling a search never has to write and two readers at the same
/// instant see the same status.
#[derive(Debug, Clone)]
pub struct SearchRecord {
    /// The workspace the search runs against. A search is addressed by
    /// `(workspaceName, searchId)`, so the pair is its identity.
    pub workspace_name: String,
    pub search_id: String,
    /// The `queryStatement` the call carried, which `DescribeSearch` reports
    /// back and a results read answers from.
    pub query_statement: String,
    /// The search strategy this search runs under: the one the call named, or
    /// [`DEFAULT_SEARCH_TYPE`] when it named none.
    pub search_type: String,
    /// The filters the call carried, kept as an `Option` so a search started
    /// without any reports none rather than empty lists it never carried.
    pub search_filters: Option<SearchFilters>,
    pub group_id: Option<String>,
    /// When the call that started the search landed — what `startedAt` reports.
    pub started_at: SystemTime,
    /// The instant the owed `QUEUED` -> `RUNNING` transition falls due.
    pub running_at: SystemTime,
    /// The instant the owed `RUNNING` -> `SUCCEEDED` transition falls due.
    pub succeeded_at: SystemTime,
}

impl SearchRecord {
    /// The search's `SearchStatus` as of `now`: queued until its run falls due,
    /// running until its run finishes, succeeded from then on. Derived, so a
    /// poll never has to write.
    ///
    /// No run of this emulator fails, so `FAILED` — and the `statusReason` the
    /// model documents as "Populated when the search has FAILED" — is a status
    /// nothing here reports.
    ///
    /// This status is a RECORD of the lifecycle the start owed, not a report on
    /// work something performed: the search's query is persisted but nothing
    /// evaluates it here. The results read
    /// (`crate::ops::get_search_results`) is where that query is answered, and it
    /// holds to the same rule: it DERIVES the matching segments from this store
    /// and this record's own query and filters, and never synthesises results to
    /// match the status reached here — until this status is
    /// [`SEARCH_SUCCEEDED`] it answers no results at all, the way the operation
    /// documents it.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.running_at {
            SEARCH_QUEUED
        } else if now < self.succeeded_at {
            SEARCH_RUNNING
        } else {
            SEARCH_SUCCEEDED
        }
    }
}

/// The key a search is stored under in
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::searches`]: the workspace it
/// runs against and its own id, so a search is reachable only through the
/// workspace it belongs to.
pub fn search_key(workspace_name: &str, search_id: &str) -> (String, String) {
    (workspace_name.to_string(), search_id.to_string())
}

/// The resource type section a search's ARN carries, which is what the modelled
/// conflict refusal reports for one.
pub const SEARCH_RESOURCE_TYPE: &str = "search";

/// The canonical ARN of a search in `region`.
///
/// The resource section carries the WORKSPACE as well as the id, for the reason
/// [`query_arn`] states: a search is addressed only inside the workspace it was
/// started in.
pub fn search_arn(region: &str, workspace_name: &str, search_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{SEARCH_RESOURCE_TYPE}/{workspace_name}/{search_id}"),
    )
}

/// One version of a task: the caller-owned configuration as of the call that
/// wrote it.
///
/// A task is VERSIONED — `UpdateTask` answers with "The new version of the task
/// created by this update", and `DescribeTask`'s `taskVersion` reads "The
/// version number of the task to retrieve. If not specified, returns the latest
/// version" — so an accepted write appends one of these instead of overwriting
/// the configuration in place, and a read naming an older version still answers
/// from the configuration that version carried.
#[derive(Debug, Clone)]
pub struct TaskVersion {
    /// This version's number, as the modelled `Version` string spells it.
    pub version: u64,
    pub description: Option<String>,
    /// The `TaskConfiguration` this version carries, stored exactly as the call
    /// that wrote it sent it.
    pub configuration: TaskConfiguration,
    /// When the call that wrote this version landed — what `updatedAt` reports
    /// for it.
    pub recorded_at: SystemTime,
}

/// A stored task: its identity within a workspace, the version a read answers
/// from, every version a later write superseded, and the `tags` the tagging
/// operations read.
#[derive(Debug, Clone)]
pub struct TaskRecord {
    /// The workspace this task lives in. A task name "must be unique within the
    /// workspace", so the pair is the task's identity.
    pub workspace_name: String,
    pub name: String,
    pub arn: String,
    /// The newest version: what a read naming no version answers from.
    pub current: TaskVersion,
    /// The versions a later write replaced, ascending by version number.
    pub superseded: Vec<TaskVersion>,
    pub created_at: SystemTime,
    /// The instant the transition the clock owes — from [`Self::settling_state`]
    /// to `ACTIVE` — falls due.
    pub active_at: SystemTime,
    /// The `ResourceState` this task reports until [`Self::active_at`] falls
    /// due: [`STATE_CREATING`] for a freshly created task, [`STATE_UPDATING`]
    /// while a mutating call it already accepted settles.
    pub settling_state: String,
    pub tags: HashMap<String, String>,
}

impl TaskRecord {
    /// The task's `version` as the modelled `Version` string.
    pub fn version_label(&self) -> String {
        self.current.version.to_string()
    }

    /// The version a `Version` label names — the current one or any it
    /// superseded — or `None` when no write ever produced it.
    ///
    /// Compared NUMERICALLY, after parsing the label the modelled `Version`
    /// pattern (`^(0|([1-9]{1}\d*))$`, at most 10 digits) admits.
    pub fn version(&self, label: &str) -> Option<&TaskVersion> {
        let wanted: u64 = label.parse().ok()?;
        if self.current.version == wanted {
            return Some(&self.current);
        }
        self.superseded
            .iter()
            .find(|version| version.version == wanted)
    }

    /// When the task was last written: the instant its current version was
    /// recorded.
    pub fn updated_at(&self) -> SystemTime {
        self.current.recorded_at
    }

    /// The task's `ResourceState` as of `now`: its [`Self::settling_state`]
    /// until the owed transition falls due, `ACTIVE` from then on. Derived, so a
    /// read never has to write.
    pub fn state(&self, now: SystemTime) -> &str {
        state_at(&self.settling_state, self.active_at, now)
    }

    /// The task's status as of `now`. No write has failed, so it carries no
    /// error.
    pub fn status(&self, now: SystemTime) -> ResourceStatus {
        ResourceStatus {
            state: Some(self.state(now).to_string()),
            error: None,
        }
    }

    /// The environment variables the task's current configuration itself
    /// defines — the lowest layer of the hierarchy
    /// [`resolved_environment_variables`] folds.
    pub fn environment_variables(&self) -> Option<&HashMap<String, String>> {
        match &self.current.configuration {
            TaskConfiguration::containerTaskConfiguration(container) => {
                container.environment_variables.as_ref()
            }
        }
    }

    /// The mounts the task's current configuration itself defines — what an
    /// execution's mount overrides are merged into; see [`resolved_mounts`].
    pub fn mounts(&self) -> &[Mount] {
        match &self.current.configuration {
            TaskConfiguration::containerTaskConfiguration(container) => {
                container.mounts.as_deref().unwrap_or(&[])
            }
        }
    }

    /// Record that an accepted update landed at `now`: the configuration it
    /// carries becomes a NEW version, the version it replaces is kept for a read
    /// that names it, and the task reports `UPDATING` until the transition this
    /// call owes falls due.
    ///
    /// The due instant is STORED, never slept on, so the caller's own call
    /// returns immediately and the transient state is still what a read taken
    /// straight afterwards observes.
    pub fn record_update(
        &mut self,
        description: Option<String>,
        configuration: TaskConfiguration,
        now: SystemTime,
    ) {
        let next = TaskVersion {
            version: self.current.version + 1,
            description,
            configuration,
            recorded_at: now,
        };
        let previous = ::std::mem::replace(&mut self.current, next);
        self.superseded.push(previous);
        // The retained history is BOUNDED: past [`MAX_TASK_VERSIONS`] the oldest
        // superseded version is evicted, the way a pipeline's history is, so no
        // number of updates grows one record without limit.
        let retained = self.superseded.len() + 1;
        if retained > MAX_TASK_VERSIONS {
            self.superseded.drain(..retained - MAX_TASK_VERSIONS);
        }
        self.settling_state = STATE_UPDATING.to_string();
        self.active_at = now + ACTIVATION_DELAY;
    }
}

/// The key a task is stored under in
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::tasks`]: the workspace it lives
/// in and its own name, since the name is unique only within the workspace.
pub fn task_key(workspace_name: &str, task_name: &str) -> (String, String) {
    (workspace_name.to_string(), task_name.to_string())
}

/// The canonical ARN of a task in `region`.
///
/// The resource section carries the WORKSPACE as well as the task name, for the
/// reason [`pipeline_arn`] states: the name is unique only within its workspace,
/// so two workspaces may each hold a task of one name and the ARN has to tell
/// them apart.
pub fn task_arn(region: &str, workspace_name: &str, task_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{TASK_RESOURCE_TYPE}/{workspace_name}/{task_name}"),
    )
}

/// The `TargetResourceType` value that addresses an asset. Source:
/// `smithy_query(query="shape", shape="TargetResourceType")` — ASSET |
/// COMPUTATION_MODEL.
pub const TARGET_RESOURCE_TYPE_ASSET: &str = "ASSET";

/// The `TargetResourceType` value that addresses a computation model. See
/// [`TARGET_RESOURCE_TYPE_ASSET`] for the source of the pair.
pub const TARGET_RESOURCE_TYPE_COMPUTATION_MODEL: &str = "COMPUTATION_MODEL";

/// The `ExecutionState` an execution reports while the action it runs is still in
/// flight.
///
/// Source: `smithy_query(query="shape", shape="ExecutionState")` — RUNNING |
/// COMPLETED | FAILED, of which this is the only non-terminal value.
pub const EXECUTION_STATE_RUNNING: &str = "RUNNING";

/// The `ExecutionState` of an execution whose action finished successfully. See
/// [`EXECUTION_STATE_RUNNING`] for the source of the set.
pub const EXECUTION_STATE_COMPLETED: &str = "COMPLETED";

/// The `ExecutionState` of an execution whose action finished unsuccessfully. See
/// [`EXECUTION_STATE_RUNNING`] for the source of the set.
pub const EXECUTION_STATE_FAILED: &str = "FAILED";

/// Every `ExecutionState` an execution can report, in the order the modelled enum
/// declares them.
///
/// Source: `smithy_query(query="shape", shape="ExecutionState")` — RUNNING |
/// COMPLETED | FAILED. This is the catalog the execution summary of a computation
/// model is keyed by, so a summary reports every state the model declares rather
/// than only those some execution happens to be in.
pub const EXECUTION_STATES: [&str; 3] = [
    EXECUTION_STATE_RUNNING,
    EXECUTION_STATE_COMPLETED,
    EXECUTION_STATE_FAILED,
];

/// How long after the call that starts it an execution of an action takes to
/// settle: until then it reports [`EXECUTION_STATE_RUNNING`], and its outcome
/// from then on.
///
/// CHOSEN, not documented: AWS publishes no duration for an action's run, only
/// that the call starting one is ACCEPTED (HTTP 202) and that an execution
/// reports `RUNNING` while "the process" has not ended. The span is deliberately
/// non-zero, so a caller that reads the execution straight after the call still
/// observes the running state the real service reports.
pub const ACTION_EXECUTION_DELAY: Duration = Duration::from_millis(500);

/// The ceiling on how many actions this store holds.
///
/// Not a published AWS quota: the store is in memory, an execute is the only
/// thing that adds an action and nothing evicts one, so this is where the
/// emulator stops growing — the quota refusal `ExecuteAction` models
/// (`LimitExceededException`, "You've reached the quota for a resource"). Each
/// action starts exactly one execution, so this bounds the executions too.
pub const MAX_ACTIONS: usize = 1_000;

/// The members of the `TargetResource` shape, in the order it declares them.
/// Source: `smithy_query(query="shape", shape="TargetResource")`.
pub const TARGET_RESOURCE_MEMBERS: [&str; 2] = ["assetId", "computationModelId"];

/// The `(TargetResourceType, ID)` pair `target_resource` names, or the refusal a
/// structure naming no single resource draws.
///
/// `TargetResource` is a choice spelled as a structure of optional members —
/// "The resource the action will be taken on. This can include asset-based
/// resources and computation model resources" — so a request naming neither of
/// them, or both, names no one resource to act on. The pair this returns is the
/// same pair the listings scoped to a target resource take, so a resource is
/// addressed identically whether the request states it as a structure or as a
/// type and an id. The `Err` carries the refusal message, which the calling
/// operation lifts into its own modelled client error.
pub fn target_resource_reference(
    target_resource: &TargetResource,
) -> Result<(&'static str, &str), String> {
    match (
        target_resource.asset_id.as_deref(),
        target_resource.computation_model_id.as_deref(),
    ) {
        (Some(asset_id), None) => Ok((TARGET_RESOURCE_TYPE_ASSET, asset_id)),
        (None, Some(computation_model_id)) => Ok((
            TARGET_RESOURCE_TYPE_COMPUTATION_MODEL,
            computation_model_id,
        )),
        (asset_id, computation_model_id) => {
            let named =
                usize::from(asset_id.is_some()) + usize::from(computation_model_id.is_some());
            Err(format!(
                "targetResource must name exactly one of {}; this request named {named}.",
                catalog_names(TARGET_RESOURCE_MEMBERS.into_iter())
            ))
        }
    }
}

/// The member a request states the TYPE of the resolved resource with. Source:
/// `smithy_query(query="operation", operation="ListActions")` —
/// `resolveToResourceType (ResolveToResourceType, one of: ASSET)`.
pub const RESOLVE_TO_RESOURCE_TYPE_MEMBER: &str = "resolveToResourceType";

/// The member a request states the ID of the resolved resource with. See
/// [`RESOLVE_TO_RESOURCE_TYPE_MEMBER`] for the source of the pair.
pub const RESOLVE_TO_RESOURCE_ID_MEMBER: &str = "resolveToResourceId";

/// Why a `(resolveToResourceType, resolveToResourceId)` pair names no one
/// resolved resource.
///
/// Described structurally rather than as one error type, for the reason
/// [`TripleRefusal`] gives: each operation taking the pair declares its own
/// modelled error set and maps these onto its own variants — a pair stated by
/// halves onto its invalid-request error, an absent resource onto its not-found
/// error — while [`ResolveToRefusal::message`] is the shared wording.
#[derive(Debug, Clone)]
pub enum ResolveToRefusal {
    /// Only one half of the pair was stated, so it addresses nothing: `stated`
    /// is the member the request gave, `missing` the one it left out.
    StatedByHalves {
        stated: &'static str,
        missing: &'static str,
    },
    /// The pair addresses an asset this store does not hold.
    AssetNotFound(String),
}

impl ResolveToRefusal {
    /// What the caller is told.
    pub fn message(&self) -> String {
        match self {
            Self::StatedByHalves { stated, missing } => {
                format!("{missing} is required when {stated} is specified.")
            }
            Self::AssetNotFound(asset_id) => asset_not_found_message(asset_id),
        }
    }
}

impl ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// The asset a `(resolveToResourceType, resolveToResourceId)` pair addresses,
    /// `None` when the request stated neither member, or the refusal the pair
    /// draws.
    ///
    /// The pair is a resource reference — a type and an id that TOGETHER name one
    /// resolved resource — so half of it addresses nothing and is refused as the
    /// bad request it is, and an id this store holds no asset for draws the same
    /// absence refusal every other addressed resource does.
    /// `ResolveToResourceType` models the single value `ASSET`, which the runtime
    /// checks before any handler runs, so the id names an asset.
    ///
    /// The single definition for every operation that narrows to a resolved
    /// resource — the two listings scoped to a target resource and a computation
    /// model's execution summary — so a resolved resource is addressed
    /// identically wherever a request names one, exactly as
    /// [`target_resource_reference`] does for the sibling reference.
    pub fn resolve_to_asset<'a>(
        &self,
        resolve_to_resource_type: Option<&str>,
        resolve_to_resource_id: Option<&'a str>,
    ) -> Result<Option<&'a str>, ResolveToRefusal> {
        match (resolve_to_resource_type, resolve_to_resource_id) {
            (Some(_), Some(asset_id)) => {
                if self.assets.contains_key(asset_id) {
                    Ok(Some(asset_id))
                } else {
                    Err(ResolveToRefusal::AssetNotFound(asset_id.to_string()))
                }
            }
            (None, None) => Ok(None),
            (Some(_), None) => Err(ResolveToRefusal::StatedByHalves {
                stated: RESOLVE_TO_RESOURCE_TYPE_MEMBER,
                missing: RESOLVE_TO_RESOURCE_ID_MEMBER,
            }),
            (None, Some(_)) => Err(ResolveToRefusal::StatedByHalves {
                stated: RESOLVE_TO_RESOURCE_ID_MEMBER,
                missing: RESOLVE_TO_RESOURCE_TYPE_MEMBER,
            }),
        }
    }
}

/// One execution of an action on a target resource: what `ListExecutions`
/// summarises, keyed by its own `executionId` in
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::executions`].
///
/// An execution runs for a span rather than finishing inside the call that starts
/// it, so the record keeps the instant its run is owed to end
/// ([`Self::settles_at`]) and the terminal state it settles into
/// ([`Self::outcome`]) instead of a state field a read would have to move:
/// [`Self::state`] reports [`EXECUTION_STATE_RUNNING`] until that instant falls
/// due and the outcome from then on, so a listing never has to write to report
/// what the clock already made true.
#[derive(Debug, Clone)]
pub struct ExecutionRecord {
    /// The id this execution is addressed by.
    pub execution_id: String,
    /// The `actionType` of the action definition this execution runs, which is
    /// also what the listing's `actionType` filter matches on.
    pub action_type: Option<String>,
    /// The resource the action was taken on: an asset or a computation model.
    pub target_resource: TargetResource,
    /// The version of the target resource the action ran against.
    pub target_resource_version: String,
    /// The detailed resource this execution resolves to, which only an execution
    /// whose target resolves through a data binding carries.
    pub resolve_to: Option<ResolveTo>,
    /// When the execution started.
    pub execution_start_time: SystemTime,
    /// The instant this execution's run is owed to end — when it stops reporting
    /// [`EXECUTION_STATE_RUNNING`] and reports [`Self::outcome`] instead.
    pub settles_at: SystemTime,
    /// The terminal `ExecutionState` this execution settles into:
    /// [`EXECUTION_STATE_COMPLETED`] or [`EXECUTION_STATE_FAILED`].
    pub outcome: String,
    /// The execution entity version associated with the summary.
    pub execution_entity_version: Option<String>,
}

impl ExecutionRecord {
    /// The `ExecutionState` this execution reports at `now`: still running until
    /// [`Self::settles_at`] falls due, its outcome from then on.
    pub fn state(&self, now: SystemTime) -> &str {
        if now < self.settles_at {
            EXECUTION_STATE_RUNNING
        } else {
            &self.outcome
        }
    }

    /// When the execution ended, which is reported only once it has: "The time
    /// the process ended" is absent for one still running.
    pub fn execution_end_time(&self, now: SystemTime) -> Option<SystemTime> {
        (now >= self.settles_at).then_some(self.settles_at)
    }

    /// `true` when `resource_type` and `resource_id` address THIS execution's
    /// target resource — the pairing `ListExecutions` scopes its listing by.
    pub fn targets(&self, resource_type: &str, resource_id: &str) -> bool {
        target_resource_addresses(&self.target_resource, resource_type, resource_id)
    }

    /// `true` when this execution NAMES the asset `asset_id` — through its
    /// target resource or through the resource it resolves to, as
    /// [`references_asset`] states it for the action it runs too.
    pub fn references_asset(&self, asset_id: &str) -> bool {
        references_asset(&self.target_resource, self.resolve_to.as_ref(), asset_id)
    }

    /// This execution's status at `now`: the state [`Self::state`] derives, which
    /// is the one member the modelled status carries.
    ///
    /// Here rather than in a handler so the read of ONE execution and the listing
    /// that holds it report the same state for it.
    pub fn status(&self, now: SystemTime) -> ExecutionStatus {
        ExecutionStatus {
            state: self.state(now).to_string(),
        }
    }

    /// This execution as the summary a listing reports for it, at `now`.
    ///
    /// Derived here rather than in the handler so every view of one stored
    /// execution reports the same state, end time and versions.
    pub fn summary(&self, now: SystemTime) -> ExecutionSummary {
        ExecutionSummary {
            execution_id: self.execution_id.clone(),
            action_type: self.action_type.clone(),
            target_resource: self.target_resource.clone(),
            target_resource_version: self.target_resource_version.clone(),
            resolve_to: self.resolve_to.clone(),
            execution_start_time: to_date_time(self.execution_start_time),
            execution_end_time: self.execution_end_time(now).map(to_date_time),
            execution_status: self.status(now),
            execution_entity_version: self.execution_entity_version.clone(),
        }
    }
}

impl ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// The execution summary of the computation model `computation_model_id`, as
    /// of `now`: how many of the executions this store holds against that model
    /// are in each `ExecutionState`, as the decimal counts the modelled summary
    /// values are strings of.
    ///
    /// `resolved_asset_id` narrows the summary to the executions that resolve to
    /// ONE asset, which is what a request naming a resolved resource asks the
    /// summary to cover.
    ///
    /// The counts are DERIVED from the stored executions on every read, through
    /// the one [`ExecutionRecord::state`] rule a listing reports an execution by,
    /// so a summary follows what the clock has made of each run — an execution
    /// counts as running until its run is due to end and under its outcome from
    /// then on — and reading one writes nothing.
    ///
    /// The keys are [`EXECUTION_STATES`] and the values decimal counts, both of
    /// which the modelled `ComputationModelExecutionSummaryKey` and
    /// `ComputationModelExecutionSummaryValue` patterns admit.
    pub fn computation_model_execution_summary(
        &self,
        computation_model_id: &str,
        resolved_asset_id: Option<&str>,
        now: SystemTime,
    ) -> HashMap<String, String> {
        let mut counts: HashMap<&str, u64> =
            EXECUTION_STATES.iter().map(|state| (*state, 0)).collect();
        let covered = self.executions.values().filter(|record| {
            record.targets(TARGET_RESOURCE_TYPE_COMPUTATION_MODEL, computation_model_id)
                && resolved_asset_id.is_none_or(|asset_id| {
                    record
                        .resolve_to
                        .as_ref()
                        .is_some_and(|resolve_to| resolve_to.asset_id == asset_id)
                })
        });
        for record in covered {
            *counts.entry(record.state(now)).or_default() += 1;
        }
        counts
            .into_iter()
            .map(|(state, count)| (state.to_string(), count.to_string()))
            .collect()
    }

    /// The DISTINCT resources the executed actions of the computation model
    /// `computation_model_id` resolved to, as the asset ids that name them, in
    /// ascending id order.
    ///
    /// Executing an action on a resource leaves two records of it in this store —
    /// the action taken on that resource and the run of that action — and each
    /// carries the detailed resource the action resolved to, so both are read
    /// here and the ids are de-duplicated: a model acted on many times against
    /// one asset resolves to that ONE resource, and an action resolving to
    /// nothing contributes none. The order is the id's own, which is unique
    /// within the enumeration and stable across calls, so it also serves as the
    /// resume point of the listing that pages this.
    ///
    /// Derived from the stored records on every read, so nothing is written to
    /// report what the executes already made true.
    pub fn computation_model_resolved_assets(&self, computation_model_id: &str) -> Vec<String> {
        let from_executions = self
            .executions
            .values()
            .filter(|record| {
                record.targets(TARGET_RESOURCE_TYPE_COMPUTATION_MODEL, computation_model_id)
            })
            .filter_map(|record| record.resolve_to.as_ref());
        let from_actions = self
            .actions
            .values()
            .filter(|record| {
                record.targets(TARGET_RESOURCE_TYPE_COMPUTATION_MODEL, computation_model_id)
            })
            .filter_map(|record| record.resolve_to.as_ref());
        from_executions
            .chain(from_actions)
            .map(|resolve_to| resolve_to.asset_id.clone())
            .collect::<BTreeSet<String>>()
            .into_iter()
            .collect()
    }
}
