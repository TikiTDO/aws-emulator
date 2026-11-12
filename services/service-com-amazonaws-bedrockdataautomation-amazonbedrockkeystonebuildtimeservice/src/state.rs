// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice service.
//!
//! This module owns the Blueprint record the control-plane operations
//! (`CreateBlueprint`, `GetBlueprint`, `ListBlueprints`, `UpdateBlueprint`,
//! `DeleteBlueprint`, `CreateBlueprintVersion`, `CopyBlueprintStage`) share and the
//! DataAutomationProject
//! record `CreateDataAutomationProject`, `GetDataAutomationProject` and
//! `UpdateDataAutomationProject` share, the DataAutomationLibrary record
//! `CreateDataAutomationLibrary` mints and `GetDataAutomationLibrary` reports,
//! and the blueprint-optimization job record
//! `InvokeBlueprintOptimizationAsync` mints and `GetBlueprintOptimizationStatus`
//! reports, plus
//! the few helpers more than one of those handlers needs: minting a resource ARN,
//! resolving the request's region, and the listing order and opaque `nextToken`
//! codec the paginated lists (`ListBlueprints`, `ListDataAutomationProjects`) share.
//! Operation BEHAVIOUR stays in `src/ops/<operation>.rs`; only the
//! shared records and their projections live here.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Blueprint, BlueprintItem, BlueprintOptimizationOutputConfiguration,
    BlueprintOptimizationSample, BlueprintSummary, CustomOutputConfiguration,
    DataAutomationLibrary, DataAutomationLibraryConfiguration, DataAutomationLibraryItem,
    DataAutomationLibrarySummary, DataAutomationProject, DataAutomationProjectSummary,
    EntityTypeInfo, OverrideConfiguration, StandardOutputConfiguration, Tag,
};

/// How a project's STORED configuration becomes the EFFECTIVE configuration a read
/// reports: the service's defaults materialised wherever the create or the update was
/// silent. Used by [`DataAutomationProjectRecord::to_project`].
pub mod project_configuration;

/// Which ARN addresses a taggable resource of this service, where that resource's tags
/// live in this store, and how a tagging call changes them. Used by the `TagResource`,
/// `UntagResource` and `ListTagsForResource` handlers.
pub mod tagging;

/// Which `dataAutomationProfileArn` a submission can be run under: the Data Automation
/// profiles this service resolves one against. Used by
/// `InvokeBlueprintOptimizationAsync`.
pub mod data_automation_profile;

/// The data automation library ingestion job record, its clock-owed status and the two
/// projections a read and a listing serve. Used by
/// `InvokeDataAutomationLibraryIngestionJob`, `GetDataAutomationLibraryIngestionJob` and
/// `ListDataAutomationLibraryIngestionJobs`.
pub mod library_ingestion;

/// The `blueprintStage` a blueprint takes when `CreateBlueprint` names none.
///
/// `CreateBlueprintRequest$blueprintStage` is OPTIONAL and targets the enum
/// `BlueprintStage` (`smithy_query(shape="BlueprintStage")` -> `["DEVELOPMENT",
/// "LIVE"]`), so the service picks the stage when the request does not: a create
/// that names no stage comes back reporting `LIVE`, and every later read of that
/// blueprint reports the same stage. A blueprint is only at `DEVELOPMENT` when its
/// create (or a later `UpdateBlueprint`) asked for that stage by name.
pub const DEFAULT_BLUEPRINT_STAGE: &str = "LIVE";

/// The region an ARN carries when the request names none (unsigned/local calls).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace Bedrock Data Automation resources live under
/// (`OPERATIONS.md`: "ARN namespace: bedrock").
pub const ARN_SERVICE: &str = "bedrock";

/// The largest page `ListBlueprints` serves, and the page size it uses when the
/// request names no `maxResults`: `ListBlueprintsRequest$maxResults` is modelled
/// `@range(min: 1, max: 1000)`, so no caller can ask for more than this and an
/// absent `maxResults` cannot serialise the whole store either.
pub const BLUEPRINT_PAGE_LIMIT: i32 = 1_000;

/// The largest page `ListDataAutomationProjects` serves, and the page size it uses
/// when the request names no `maxResults`. The same value as
/// [`BLUEPRINT_PAGE_LIMIT`] because it has the same source: both listings bind
/// `maxResults` to the one modelled `MaxResults` shape
/// (`smithy_query(operation="ListDataAutomationProjects")` -> `@range(min: 1, max:
/// 1000)`), so neither can be asked for more than this and an absent `maxResults`
/// cannot serialise the whole store either.
pub const PROJECT_PAGE_LIMIT: i32 = BLUEPRINT_PAGE_LIMIT;

/// The `resourceOwner` value naming the CALLER's own resources, which is what a
/// listing reports when the request names no owner.
///
/// Source: `smithy_query(shape="ResourceOwner")` -> `["SERVICE", "ACCOUNT"]`; every
/// resource this store holds was created through this service's own create
/// operations, so it is ACCOUNT-owned.
pub const RESOURCE_OWNER_ACCOUNT: &str = "ACCOUNT";

/// The wildcard a stage filter takes to mean "every stage", shared by this
/// service's stage filters: `smithy_query(shape="BlueprintStageFilter")` and
/// `smithy_query(shape="DataAutomationProjectStageFilter")` both list it beside
/// `DEVELOPMENT` and `LIVE`.
pub const STAGE_FILTER_ALL: &str = "ALL";

/// How many blueprints the store holds before `CreateBlueprint` refuses with the
/// modelled `ServiceQuotaExceededException` (HTTP 402).
///
/// Bedrock Data Automation publishes an adjustable quota on blueprints per
/// account per Region, and this emulator models neither Service Quotas nor an
/// account's adjustments — so this is the emulator's OWN bound on the store
/// rather than a claim about any account's limit. It is set far above any
/// workflow an emulator serves, and it is what stops the state growing without
/// end: every record it holds is released by `DeleteBlueprint`.
///
/// It also bounds `create_idempotency`: a create inserts exactly one token entry
/// for the record it created, and `remove_blueprint` drops every token that
/// resolves to a deleted ARN, so that map holds at most one entry per live
/// blueprint.
pub const BLUEPRINT_QUOTA: usize = 1_000;

/// How many schema snapshots one blueprint holds before
/// `CreateBlueprintVersion` refuses with the modelled
/// `ServiceQuotaExceededException` (HTTP 402). Same reasoning as
/// [`BLUEPRINT_QUOTA`]: a version cut copies the whole live schema (modelled up
/// to 100000 characters), so the per-blueprint snapshot count needs its own
/// bound. `DeleteBlueprint` with a `blueprintVersion` releases one snapshot and
/// without one releases them all.
///
/// Together with [`BLUEPRINT_QUOTA`] it bounds `version_idempotency` too: a cut
/// inserts one token entry per snapshot it takes, and every path that releases a
/// snapshot ([`ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState::forget_version_token`]
/// on a versioned delete,
/// [`ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState::remove_blueprint`]
/// on a whole-blueprint delete) releases its token with it, so that map holds at
/// most one entry per live snapshot.
pub const BLUEPRINT_VERSION_QUOTA: usize = 100;

/// The resource portion prefix of a blueprint-optimization invocation ARN, as the
/// modelled `BlueprintOptimizationInvocationArn` pattern spells it
/// (`smithy_query(operation="GetBlueprintOptimizationStatus")`:
/// `…:blueprint-optimization-invocation/[a-zA-Z0-9-_]+`).
pub const OPTIMIZATION_INVOCATION_RESOURCE_PREFIX: &str = "blueprint-optimization-invocation/";

/// The status a blueprint-optimization job carries from the moment
/// `InvokeBlueprintOptimizationAsync` accepts it.
///
/// Source: `smithy_query(shape="BlueprintOptimizationJobStatus")` ->
/// `["Created", "InProgress", "Success", "ServiceError", "ClientError"]`. The two error
/// statuses are reached by no path here: nothing in this emulator fails a job it has
/// accepted, and a submission this service refuses is refused on the wire rather than
/// recorded as a failed job.
pub const OPTIMIZATION_CREATED: &str = "Created";

/// The status a blueprint-optimization job carries once the clock has made its first
/// transition due. See [`OPTIMIZATION_CREATED`] for the source of the value.
pub const OPTIMIZATION_IN_PROGRESS: &str = "InProgress";

/// The terminal status of a blueprint-optimization job this store accepted. See
/// [`OPTIMIZATION_CREATED`] for the source of the value.
///
/// It says that the job RECORD reached its terminal state; it does not claim that an
/// optimized blueprint exists under the submitted `outputConfiguration.s3Object`, which
/// lives in a service this crate does not emulate.
pub const OPTIMIZATION_SUCCESS: &str = "Success";

/// How long after `InvokeBlueprintOptimizationAsync` the clock owes a job its
/// `Created` -> `InProgress` transition, and how long after that instant it owes
/// `InProgress` -> `Success`.
///
/// AWS documents no duration for either leg — `GetBlueprintOptimizationStatus` models
/// `status` as one of `Created`/`InProgress`/`Success` precisely because an accepted job
/// is not finished when the submission returns — so this span is CHOSEN, not modelled.
/// It is deliberately non-zero, so a caller polling straight after the submission still
/// observes `Created` and the status it reports is never skipped. Nothing sleeps for it:
/// the two due instants are stored on the record and compared against the clock.
pub const OPTIMIZATION_STAGE_SPAN: Duration = Duration::from_secs(3);

/// How many blueprint-optimization jobs the store holds before
/// `InvokeBlueprintOptimizationAsync` refuses with the modelled
/// `ServiceQuotaExceededException` (HTTP 402).
///
/// Same reasoning as [`BLUEPRINT_QUOTA`]: Bedrock Data Automation publishes adjustable
/// quotas per account per Region and this emulator models neither Service Quotas nor an
/// account's adjustments, so this is the emulator's OWN bound on the store — what stops
/// a caller submitting jobs from growing the state without end.
pub const OPTIMIZATION_JOB_QUOTA: usize = 1_000;

/// What this service answers when an `invocationArn` names no blueprint-optimization
/// job this store holds.
///
/// The "operation" this refusal speaks of is the ASYNCHRONOUS JOB the ARN names, not the
/// API call: the answer is this service's own modelled `ValidationException`, which it
/// raises only after it has understood the request. The text is fixed and carries
/// NOTHING from the request — the caller's ARN is not echoed back, exactly as this
/// service's other refusals for a resource it cannot resolve do not echo theirs
/// ([`BLUEPRINT_ARN_NOT_FOUND_MESSAGE`]).
pub const UNKNOWN_INVOCATION_MESSAGE: &str =
    "The requested operation is not recognized by the service.";

/// What this service answers when an ARN it was handed names no BLUEPRINT this store
/// holds — whichever operation was handed it, and whether it named the blueprint
/// directly (`blueprintArn`) or by reference from a filter or a submission.
///
/// The text is FIXED: it names the resource kind and the member class that failed to
/// resolve, and takes nothing from the request or from state, so the refusal a caller
/// branches on is the same sentence however long the ARN it sent was. Single-homed here
/// because every operation of this service that resolves a blueprint states the same
/// refusal, and two copies of a fixed sentence are two sentences waiting to drift.
pub const BLUEPRINT_ARN_NOT_FOUND_MESSAGE: &str = "Blueprint Arn not found.";

/// What this service answers when the request SCOPED a blueprint lookup to a version and
/// the ARN-and-version PAIR resolves to nothing — whether it is the ARN or the version
/// that is not there.
///
/// A version-scoped lookup names two things, and the refusal speaks of both rather than
/// of the ARN alone, because either half can be the one that does not resolve. Fixed
/// text, like the unscoped sentence above, with nothing taken from the request.
pub const BLUEPRINT_ARN_OR_VERSION_INVALID_MESSAGE: &str =
    "Incorrect Blueprint Arn or Version provided in the request.";

/// The sentence a blueprint lookup that resolved to nothing is refused with, chosen by
/// whether the request scoped the lookup to a version (`blueprintVersion` present, at
/// whatever value) rather than by which half of the pair was missing.
///
/// The ONE place that choice is made, so the operations that accept a `blueprintVersion`
/// (`GetBlueprint`, `DeleteBlueprint`) and the ones that do not cannot drift into
/// different sentences for the same refusal.
#[must_use]
pub fn blueprint_not_found_message(blueprint_version: Option<&str>) -> &'static str {
    match blueprint_version {
        Some(_) => BLUEPRINT_ARN_OR_VERSION_INVALID_MESSAGE,
        None => BLUEPRINT_ARN_NOT_FOUND_MESSAGE,
    }
}

/// [`BLUEPRINT_ARN_NOT_FOUND_MESSAGE`] for the other resource this store holds: what
/// this service answers when an ARN names no DATA AUTOMATION PROJECT it holds. The
/// resource kind is named the way this service names it in a refusal (`Project`), and
/// nothing is taken from the request.
pub const PROJECT_ARN_NOT_FOUND_MESSAGE: &str = "Project Arn not found.";

/// What this service answers when `CreateBlueprint` is handed a `blueprintName` some
/// live blueprint already holds.
///
/// Fixed text, naming the resource kind and the member whose value is taken, with the
/// requested name NOT interpolated: `BlueprintName` is modelled `@sensitive`
/// (`smithy_query(shape="BlueprintName")`), so echoing it would put a value the model
/// says not to disclose into every client log the refusal reaches.
pub const BLUEPRINT_NAME_CONFLICT_MESSAGE: &str = "Blueprint name already exists.";

/// [`BLUEPRINT_NAME_CONFLICT_MESSAGE`] for a project: what
/// `CreateDataAutomationProject` answers when some live project already holds the
/// `projectName` it was handed. `DataAutomationProjectName` is `@sensitive` too, so the
/// requested name is not echoed here either.
pub const PROJECT_NAME_CONFLICT_MESSAGE: &str = "Project name already exists.";

/// The ARN account field AWS's OWN resources carry, as the modelled `BlueprintArn`
/// pattern admits beside a 12-digit account
/// (`…:(aws|[0-9]{12}):blueprint/(bedrock-data-automation-public-…|…)`).
const SERVICE_OWNED_ACCOUNT: &str = "aws";

/// Whether `arn` names a resource an ACCOUNT holds rather than one of AWS's own.
///
/// Read through the shared `service_common::arn` parse, so this service and its siblings
/// agree on what an ARN's account field is. Absence of an AWS-owned resource cannot be
/// concluded from this store: it holds no catalog of the public, service-owned
/// blueprints the `BlueprintArn` pattern admits.
fn names_account_owned_resource(arn: &str) -> bool {
    ::service_common::arn::parse(arn).is_some_and(|parsed| parsed.account != SERVICE_OWNED_ACCOUNT)
}

/// A stored blueprint-optimization job: the submission
/// `InvokeBlueprintOptimizationAsync` accepted, plus the two instants the clock owes
/// this job's status transitions.
#[derive(Debug, Clone)]
pub struct BlueprintOptimizationJobRecord {
    pub invocation_arn: String,
    pub blueprint_arn: String,
    /// The `blueprint.stage` the submission selected, as submitted; `None` when it
    /// named none.
    pub blueprint_stage: Option<String>,
    pub samples: Vec<BlueprintOptimizationSample>,
    pub output_configuration: BlueprintOptimizationOutputConfiguration,
    pub data_automation_profile_arn: String,
    pub kms_key_id: Option<String>,
    pub kms_encryption_context: Option<HashMap<String, String>>,
    pub tags: Vec<Tag>,
    /// When this job stops being `Created`.
    pub in_progress_at: SystemTime,
    /// When this job reaches [`OPTIMIZATION_SUCCESS`].
    pub succeeds_at: SystemTime,
}

impl BlueprintOptimizationJobRecord {
    /// This job's modeled `status` as of `now`, derived from the two stored due
    /// instants.
    ///
    /// A pure function of the record, so reporting the status writes nothing and two
    /// readers at the same instant agree.
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        if now >= self.succeeds_at {
            OPTIMIZATION_SUCCESS
        } else if now >= self.in_progress_at {
            OPTIMIZATION_IN_PROGRESS
        } else {
            OPTIMIZATION_CREATED
        }
    }
}

/// The `projectStage` a DataAutomationProject takes when the create request names
/// none.
///
/// Source: `smithy_query(shape="DataAutomationProjectStage")` ->
/// `["DEVELOPMENT", "LIVE"]`, and the member carries NO `@default`, so this value
/// is the emulator's own choice rather than a modelled one. It is `LIVE` because
/// that is the stage a project has to be in to be usable without naming one: the
/// runtime service resolves a project through
/// `DataAutomationConfiguration$stage` (`smithy_query(shape=
/// "DataAutomationConfiguration")` on the Bedrock Data Automation Runtime model),
/// which is optional too, so a project created with no stage named and invoked
/// with no stage named must be the same project.
pub const DEFAULT_PROJECT_STAGE: &str = "LIVE";

/// The two non-failed statuses of a DataAutomationProject, as
/// `smithy_query(shape="DataAutomationProjectStatus")` lists them (`COMPLETED`,
/// `IN_PROGRESS`, `FAILED`): a create answers `IN_PROGRESS` and the project
/// settles into `COMPLETED`.
pub const PROJECT_IN_PROGRESS: &str = "IN_PROGRESS";

/// See [`PROJECT_IN_PROGRESS`].
pub const PROJECT_COMPLETED: &str = "COMPLETED";

/// How long after `CreateDataAutomationProject` — and after an
/// `UpdateDataAutomationProject`, which re-specifies what the project produces and
/// so re-starts the same build — the clock owes the project's `IN_PROGRESS` ->
/// `COMPLETED` transition.
///
/// AWS documents no duration for it — `CreateDataAutomationProjectResponse$status`
/// is modelled as one of `IN_PROGRESS`/`COMPLETED`/`FAILED` precisely because the
/// project is not ready when the call returns — so this span is CHOSEN, not
/// modelled: 500 ms, inside the project's 250-750 ms band. It is deliberately
/// non-zero, so a caller reading the project straight after the create still
/// observes `IN_PROGRESS`. Nothing sleeps for it: the due instant is stored on the
/// record and compared against the clock.
pub const PROJECT_CREATION_SPAN: Duration = Duration::from_millis(500);

/// The status `DeleteDataAutomationProject` reports for the teardown it has just
/// accepted.
///
/// `DeleteDataAutomationProjectResponse$status` is modelled as a
/// `DataAutomationProjectStatus` (`smithy_query(shape="DataAutomationProjectStatus")`
/// -> `["COMPLETED", "IN_PROGRESS", "FAILED"]`), which is the one place the delete
/// answers about the teardown rather than about the project: the call ACCEPTS the
/// teardown and the teardown proceeds after it returns, so the status that holds at
/// the instant of the response is the in-flight one. A delete can therefore never
/// answer `COMPLETED` — there is no span, however short, in which the work it started
/// has already finished when it answers.
pub const PROJECT_DELETION_ACCEPTED: &str = PROJECT_IN_PROGRESS;

/// How many DataAutomationProjects the store holds before
/// `CreateDataAutomationProject` refuses with the modelled
/// `ServiceQuotaExceededException` (HTTP 402). Same reasoning as
/// [`BLUEPRINT_QUOTA`]: Bedrock Data Automation publishes an adjustable quota on
/// projects per account per Region and this emulator models neither Service Quotas
/// nor an account's adjustments, so this is the emulator's OWN bound on the store.
pub const PROJECT_QUOTA: usize = 1_000;

/// A stored DataAutomationProject: exactly the members the modeled
/// `DataAutomationProject` shape carries, plus the tag list and the instant the
/// clock owes this project's `IN_PROGRESS` -> `COMPLETED` transition.
#[derive(Debug, Clone)]
pub struct DataAutomationProjectRecord {
    pub project_arn: String,
    pub project_name: String,
    pub project_stage: String,
    pub project_type: String,
    pub project_description: Option<String>,
    pub standard_output_configuration: Option<StandardOutputConfiguration>,
    pub custom_output_configuration: Option<CustomOutputConfiguration>,
    pub override_configuration: Option<OverrideConfiguration>,
    pub data_automation_library_configuration: Option<DataAutomationLibraryConfiguration>,
    pub creation_time: ::aws_smithy_types::DateTime,
    pub last_modified_time: ::aws_smithy_types::DateTime,
    pub kms_key_id: Option<String>,
    pub kms_encryption_context: Option<HashMap<String, String>>,
    /// When the build this record is currently running stops being in progress. Set
    /// by the create and moved again by every update that re-specifies the
    /// project's configuration; every reader derives [`Self::status_at`] from it
    /// rather than storing a status that a read would have to write.
    pub completes_at: SystemTime,
    pub tags: Vec<Tag>,
}

impl DataAutomationProjectRecord {
    /// This project's modeled `status` as of `now`: `IN_PROGRESS` until the
    /// transition the create (or the latest update) owed falls due, `COMPLETED`
    /// from then on.
    ///
    /// A pure function of the stored due instant, so reporting it writes nothing and
    /// two readers at the same instant agree.
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        if now >= self.completes_at {
            PROJECT_COMPLETED
        } else {
            PROJECT_IN_PROGRESS
        }
    }

    /// Project the record onto the modeled `DataAutomationProject` shape, with the
    /// status the clock has made due at `now` and the project's EFFECTIVE
    /// configuration: every configuration block present, each member taking the value
    /// the create or the update specified and the service's default wherever that
    /// specification was silent (see [`project_configuration`]). A reader asks the
    /// project what it will be processed under, not which members a request happened
    /// to spell out — and the defaults are materialised HERE rather than folded into
    /// the record, so the stored configuration stays the caller's own and this
    /// projection writes nothing.
    ///
    /// INFALLIBLE: every member of the projection is a member of the record, so no
    /// request can make this fail and no handler needs an unwrap for it.
    pub fn to_project(&self, now: SystemTime) -> DataAutomationProject {
        DataAutomationProject {
            project_arn: self.project_arn.clone(),
            creation_time: self.creation_time,
            last_modified_time: self.last_modified_time,
            project_name: self.project_name.clone(),
            project_stage: Some(self.project_stage.clone()),
            project_type: Some(self.project_type.clone()),
            project_description: self.project_description.clone(),
            standard_output_configuration: Some(
                project_configuration::effective_standard_output_configuration(
                    self.standard_output_configuration.as_ref(),
                ),
            ),
            custom_output_configuration: Some(
                project_configuration::effective_custom_output_configuration(
                    self.custom_output_configuration.as_ref(),
                ),
            ),
            override_configuration: Some(project_configuration::effective_override_configuration(
                self.override_configuration.as_ref(),
            )),
            data_automation_library_configuration: Some(
                project_configuration::effective_data_automation_library_configuration(
                    self.data_automation_library_configuration.as_ref(),
                ),
            ),
            status: self.status_at(now).to_string(),
            kms_key_id: self.kms_key_id.clone(),
            kms_encryption_context: self.kms_encryption_context.clone(),
        }
    }

    /// Project the record onto the modeled `DataAutomationProjectSummary` shape that
    /// `ListDataAutomationProjects` returns.
    ///
    /// The summary carries no `status`, so a listing reports nothing a reader would
    /// have to derive from the clock: `smithy_query(shape=
    /// "DataAutomationProjectSummary")` is `projectArn`, `projectStage`,
    /// `projectType`, `projectName` and `creationTime`.
    pub fn to_summary(&self) -> DataAutomationProjectSummary {
        DataAutomationProjectSummary {
            project_arn: self.project_arn.clone(),
            project_stage: Some(self.project_stage.clone()),
            project_type: Some(self.project_type.clone()),
            project_name: Some(self.project_name.clone()),
            creation_time: self.creation_time,
        }
    }

    /// Every blueprint reference this project's configuration carries: the custom
    /// output configuration's blueprint list and the document fallbacks it names.
    /// This is what a listing's blueprint filter narrows on — a project "uses" a
    /// blueprint exactly when its stored configuration references it.
    pub fn blueprint_references(&self) -> impl Iterator<Item = &BlueprintItem> {
        let custom = self.custom_output_configuration.as_ref();
        let direct = custom.and_then(|c| c.blueprints.as_ref());
        let fallback = custom
            .and_then(|c| c.document.as_ref())
            .and_then(|d| d.fallback_blueprints.as_ref());
        direct
            .into_iter()
            .flatten()
            .chain(fallback.into_iter().flatten())
    }

    /// Every Data Automation library reference this project's configuration carries.
    /// This is what a listing's library filter narrows on.
    pub fn library_references(&self) -> impl Iterator<Item = &DataAutomationLibraryItem> {
        self.data_automation_library_configuration
            .as_ref()
            .and_then(|config| config.libraries.as_ref())
            .into_iter()
            .flatten()
    }
}

/// One snapshot of a blueprint's schema, taken by `CreateBlueprintVersion`.
#[derive(Debug, Clone)]
pub struct BlueprintVersionSnapshot {
    /// The schema as it stood when the version was cut.
    pub schema: String,
    /// When the version was cut.
    pub created_at: ::aws_smithy_types::DateTime,
}

/// What a blueprint holds AT ONE STAGE: the schema stored there and when it was
/// last written there.
///
/// A blueprint's stages are independent copies of its content — that is what
/// `CopyBlueprintStage` ("Copies a Blueprint from one stage to another",
/// `smithy_query(query="doc",shape="CopyBlueprintStage")`) copies between — so each
/// stage carries its own schema and its own `lastModifiedTime` rather than sharing
/// the record's.
#[derive(Debug, Clone)]
pub struct BlueprintStageContent {
    pub schema: String,
    pub last_modified_time: ::aws_smithy_types::DateTime,
}

/// A stored blueprint: exactly the members the modeled `Blueprint` shape carries
/// that this service owns, plus the version snapshots and the tag list.
#[derive(Debug, Clone)]
pub struct BlueprintRecord {
    pub blueprint_arn: String,
    pub blueprint_name: String,
    pub r#type: String,
    /// The schema stored at [`Self::blueprint_stage`] — the stage this blueprint
    /// was last written at.
    pub schema: String,
    /// The stage this blueprint was last WRITTEN at: the one its create named, or
    /// the one the last `UpdateBlueprint`/`CopyBlueprintStage` wrote. It is the
    /// stage a read that scopes itself to no stage reports; every other stage the
    /// blueprint exists at is in [`Self::stage_copies`].
    pub blueprint_stage: String,
    pub creation_time: ::aws_smithy_types::DateTime,
    pub last_modified_time: ::aws_smithy_types::DateTime,
    pub kms_key_id: Option<String>,
    pub kms_encryption_context: Option<HashMap<String, String>>,
    /// What this blueprint holds at the stages OTHER than
    /// [`Self::blueprint_stage`], keyed by stage: one entry per stage a write
    /// ([`Self::write_stage`], reached by `CopyBlueprintStage` and
    /// `UpdateBlueprint`) has left holding content while writing elsewhere, and
    /// no later write has taken over. A blueprint exists at a stage exactly
    /// when that stage is [`Self::blueprint_stage`] or a key of this map
    /// ([`Self::exists_at`]), which is at most one entry here because
    /// `BlueprintStage` models two values (`smithy_query(shape="BlueprintStage")`
    /// -> `["DEVELOPMENT", "LIVE"]`) and every write lands at one of them.
    pub stage_copies: BTreeMap<String, BlueprintStageContent>,
    /// Schema snapshots keyed by their version number.
    pub versions: BTreeMap<u64, BlueprintVersionSnapshot>,
    /// The version number the NEXT cut takes. Monotonic: it is never rolled back
    /// when a snapshot is deleted, so one version number never names two
    /// different schemas over the life of the record.
    pub next_version: u64,
    pub tags: Vec<Tag>,
}

impl BlueprintRecord {
    /// What this blueprint holds at `stage`, or `None` when it exists at no such
    /// stage — which every operation that scopes itself to a stage answers with
    /// the modelled 404.
    pub fn content_at(&self, stage: &str) -> Option<BlueprintStageContent> {
        if stage == self.blueprint_stage {
            return Some(BlueprintStageContent {
                schema: self.schema.clone(),
                last_modified_time: self.last_modified_time,
            });
        }
        self.stage_copies.get(stage).cloned()
    }

    /// Whether this blueprint exists at `stage` at all.
    pub fn exists_at(&self, stage: &str) -> bool {
        stage == self.blueprint_stage || self.stage_copies.contains_key(stage)
    }

    /// Project this blueprint AS IT STANDS AT ONE STAGE onto the modeled
    /// `Blueprint` shape: that stage's schema and `lastModifiedTime` over the
    /// record's own identity. `None` when it exists at no such stage.
    pub fn to_blueprint_at(&self, stage: &str) -> Option<Blueprint> {
        let content = self.content_at(stage)?;
        Some(Blueprint {
            schema: content.schema,
            last_modified_time: content.last_modified_time,
            blueprint_stage: Some(stage.to_string()),
            ..self.to_blueprint()
        })
    }

    /// Store `schema` as this blueprint's content at `stage`, written at `now`.
    ///
    /// Every OTHER stage keeps the content it holds: a write names one stage, so it
    /// cannot move another stage's schema or its `lastModifiedTime`. `stage` becomes
    /// the stage the blueprint was last written at, which is the one an unscoped read
    /// reports.
    pub fn write_stage(
        &mut self,
        stage: &str,
        schema: String,
        now: ::aws_smithy_types::DateTime,
    ) {
        if stage != self.blueprint_stage {
            // The stage the blueprint was written at until now keeps its own content,
            // as one of the OTHER stages; `stage` is the stage last written from here
            // on, so it is no longer one of them.
            let previous = BlueprintStageContent {
                schema: self.schema.clone(),
                last_modified_time: self.last_modified_time,
            };
            let previous_stage = std::mem::replace(&mut self.blueprint_stage, stage.to_string());
            self.stage_copies.insert(previous_stage, previous);
            self.stage_copies.remove(stage);
        }
        self.schema = schema;
        self.last_modified_time = now;
    }

    /// Place the content stored at `source` at `target` too, as of `now`.
    ///
    /// `false` when this blueprint exists at no `source` stage, so the caller
    /// answers the modelled 404 and nothing is written. A copy LEAVES the source
    /// stage holding its own content: after it the blueprint exists at both
    /// stages, carrying the same schema, and `target` is the stage it was last
    /// written at — so it is what an unscoped read reports. Copying a stage onto
    /// itself writes nothing: the content is already there.
    pub fn copy_stage(
        &mut self,
        source: &str,
        target: &str,
        now: ::aws_smithy_types::DateTime,
    ) -> bool {
        let Some(content) = self.content_at(source) else {
            return false;
        };
        if source != target {
            self.write_stage(target, content.schema, now);
        }
        true
    }

    /// The highest version still stored for this blueprint, if any. This is what
    /// a summary reports; the number the next cut takes is [`Self::next_version`],
    /// which does not move back when a snapshot is released.
    pub fn latest_version(&self) -> Option<u64> {
        self.versions.keys().next_back().copied()
    }

    /// Reserve the next version number for a cut, advancing the counter so the
    /// number is never handed out twice even after its snapshot is deleted.
    pub fn take_next_version(&mut self) -> u64 {
        let version = self.next_version.max(1);
        self.next_version = version.saturating_add(1);
        version
    }

    /// Project the record's LIVE (unversioned) state onto the modeled
    /// `Blueprint` shape.
    ///
    /// INFALLIBLE: every member of the projection is a member of the record, so
    /// no request can make this fail and no handler needs an unwrap for it.
    pub fn to_blueprint(&self) -> Blueprint {
        Blueprint {
            blueprint_arn: self.blueprint_arn.clone(),
            schema: self.schema.clone(),
            r#type: self.r#type.clone(),
            creation_time: self.creation_time,
            last_modified_time: self.last_modified_time,
            blueprint_name: self.blueprint_name.clone(),
            blueprint_version: None,
            blueprint_stage: Some(self.blueprint_stage.clone()),
            kms_key_id: self.kms_key_id.clone(),
            kms_encryption_context: self.kms_encryption_context.clone(),
            // A blueprint-optimization job delivers its optimized blueprint to the
            // output location its submission named, and neither the model nor the
            // service's docs say such a job re-writes the blueprint it read, so a job
            // leaves this record alone and both members stay absent. No request can set
            // them either: no input shape of this service carries them.
            optimization_samples: None,
            optimization_time: None,
        }
    }

    /// Project ONE schema snapshot of this record onto the modeled `Blueprint`
    /// shape: the versioned schema and the instant that version was cut, over
    /// the record's own identity. `None` when that version was never cut (or was
    /// deleted), which the caller answers with the modeled 404.
    pub fn to_blueprint_version(&self, version: u64) -> Option<Blueprint> {
        let snapshot = self.versions.get(&version)?;
        Some(Blueprint {
            schema: snapshot.schema.clone(),
            last_modified_time: snapshot.created_at,
            blueprint_version: Some(version.to_string()),
            ..self.to_blueprint()
        })
    }

    /// Project the record onto the NARROW `BlueprintSummary` item `ListBlueprints`
    /// serves: a listing identifies each blueprint and says when it was created, and
    /// nothing else.
    ///
    /// `BlueprintSummary` models six optional-or-required members, but a listing fills
    /// only `blueprintArn` (the identity), `blueprintVersion` (which snapshot of it the
    /// item names) and `creationTime` (@required); `blueprintStage`, `lastModifiedTime`
    /// and the `@sensitive` `blueprintName`
    /// (`smithy_query(query="shape", shape="BlueprintSummary")` marks it so) belong to
    /// the full `Blueprint` a read serves, not to an enumeration. A caller that needs
    /// any of them reads the blueprint by ARN with `GetBlueprint`.
    pub fn to_summary(&self) -> BlueprintSummary {
        BlueprintSummary {
            blueprint_arn: self.blueprint_arn.clone(),
            blueprint_version: self.latest_version().map(|v| v.to_string()),
            blueprint_stage: None,
            blueprint_name: None,
            creation_time: self.creation_time,
            last_modified_time: None,
        }
    }
}

/// The status a DataAutomationLibrary carries from the moment its create returns.
///
/// Source: `smithy_query(shape="DataAutomationLibraryStatus")` -> `["ACTIVE",
/// "DELETING"]`. A library is a container the service stands up with nothing to
/// provision, so the create answers the ready status and every later read of that
/// library answers the same: there is no non-terminal status for the clock to move
/// it out of. `DELETING` is the other half of the pair and belongs to the teardown
/// path, not to a create.
pub const LIBRARY_ACTIVE: &str = "ACTIVE";

/// How many DataAutomationLibraries the store holds before
/// `CreateDataAutomationLibrary` refuses with the modelled
/// `ServiceQuotaExceededException` (HTTP 402). Same reasoning as
/// [`BLUEPRINT_QUOTA`]: Bedrock Data Automation publishes an adjustable quota on
/// libraries per account per Region and this emulator models neither Service Quotas
/// nor an account's adjustments, so this is the emulator's OWN bound on the store.
pub const LIBRARY_QUOTA: usize = 1_000;

/// The largest page `ListDataAutomationLibraries` serves, and the page size it uses when
/// the request names no `maxResults`. The same value as [`BLUEPRINT_PAGE_LIMIT`] because it
/// has the same source: this listing binds `maxResults` to the one modelled `MaxResults`
/// shape (`smithy_query(query="operation",operation="ListDataAutomationLibraries")` ->
/// `@range(min: 1, max: 1000)`), so no caller can ask for more than this and an absent
/// `maxResults` cannot serialise the whole store either.
pub const LIBRARY_PAGE_LIMIT: i32 = BLUEPRINT_PAGE_LIMIT;

/// What `CreateDataAutomationLibrary` answers when some live library already holds
/// the `libraryName` it was handed.
///
/// AWS words this conflict as a full sentence pair telling the caller what to do
/// next, rather than in the terse form the blueprint and project conflicts take
/// ([`BLUEPRINT_NAME_CONFLICT_MESSAGE`], [`PROJECT_NAME_CONFLICT_MESSAGE`]) — so it
/// is its own sentence here, not a reuse of theirs. The text is FIXED and
/// interpolates nothing: `DataAutomationLibraryName` is `@sensitive`
/// (`smithy_query(shape="DataAutomationLibraryName")`), so the requested name is not
/// handed back to the caller.
pub const LIBRARY_NAME_CONFLICT_MESSAGE: &str =
    "A library with this name already exists. Please choose a different name.";

/// [`BLUEPRINT_ARN_NOT_FOUND_MESSAGE`] for a library: what this service answers when
/// a `libraryArn` it was handed names no DataAutomationLibrary this store holds. The
/// resource kind is named the way this service names it in a refusal (`Library`), and
/// nothing is taken from the request.
pub const LIBRARY_ARN_NOT_FOUND_MESSAGE: &str = "Library Arn not found.";

/// A stored DataAutomationLibrary: exactly the members the modeled
/// `DataAutomationLibrary` shape carries, plus the tag list the create accepted.
///
/// No status member: the only status a stored library has is [`LIBRARY_ACTIVE`]
/// (see that constant), so there is nothing per-record to keep.
#[derive(Debug, Clone)]
pub struct DataAutomationLibraryRecord {
    pub library_arn: String,
    pub library_name: String,
    pub library_description: Option<String>,
    pub creation_time: ::aws_smithy_types::DateTime,
    /// The entity types this library HOLDS, which is what a read reports. A create
    /// stands the library up empty; entities arrive through an ingestion job.
    pub entity_types: Vec<EntityTypeInfo>,
    pub kms_key_id: Option<String>,
    pub kms_encryption_context: Option<HashMap<String, String>>,
    pub tags: Vec<Tag>,
}

impl DataAutomationLibraryRecord {
    /// Project the record onto the modeled `DataAutomationLibrary` shape a read
    /// serves.
    ///
    /// INFALLIBLE: every member of the projection is a member of the record, so no
    /// request can make this fail and no handler needs an unwrap for it.
    pub fn to_library(&self) -> DataAutomationLibrary {
        DataAutomationLibrary {
            library_arn: self.library_arn.clone(),
            creation_time: self.creation_time,
            library_name: self.library_name.clone(),
            library_description: self.library_description.clone(),
            status: LIBRARY_ACTIVE.to_string(),
            // What the record HOLDS, so a library standing up empty reports an empty
            // list rather than inventing an entity type it was never given.
            entity_types: Some(self.entity_types.clone()),
            kms_key_id: self.kms_key_id.clone(),
            kms_encryption_context: self.kms_encryption_context.clone(),
        }
    }

    /// Project the record onto the NARROW `DataAutomationLibrarySummary` item
    /// `ListDataAutomationLibraries` serves.
    ///
    /// `smithy_query(query="shape",shape="DataAutomationLibrarySummary")` carries three
    /// members — the ARN (the identity), the name and the creation instant — and a listing
    /// fills all three: unlike the blueprint summary, this shape's name member is the
    /// library's own, so a caller enumerating libraries reads them without a read per
    /// library.
    pub fn to_summary(&self) -> DataAutomationLibrarySummary {
        DataAutomationLibrarySummary {
            library_arn: self.library_arn.clone(),
            library_name: Some(self.library_name.clone()),
            creation_time: self.creation_time,
        }
    }
}

/// Per-service state for com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState {
    /// Every stored blueprint, keyed by its ARN.
    pub blueprints: HashMap<String, BlueprintRecord>,
    /// `blueprintName` -> `blueprintArn`; the uniqueness index behind
    /// `CreateBlueprint`'s `ConflictException`.
    pub blueprint_names: HashMap<String, String>,
    /// `CreateBlueprint` `clientToken` -> the ARN that call created, so an
    /// `@idempotencyToken` replay returns the original blueprint.
    pub create_idempotency: HashMap<String, String>,
    /// `CreateBlueprintVersion` `clientToken` -> (ARN, version) it cut.
    pub version_idempotency: HashMap<String, (String, u64)>,
    /// The `CopyBlueprintStage` `clientToken` of the last copy served for each
    /// (`blueprintArn`, `sourceStage`, `targetStage`): what makes an
    /// `@idempotencyToken` replay of that copy a no-op rather than a second copy
    /// of whatever the source stage holds by then.
    ///
    /// Keyed by the COPY rather than by the token, so copying under fresh tokens
    /// cannot grow this map: `BlueprintStage` models two values, so it holds at
    /// most four entries per live blueprint, and
    /// [`ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState::remove_blueprint`]
    /// drops the entries of a released one.
    pub stage_copy_tokens: HashMap<(String, String, String), String>,
    /// Every stored DataAutomationProject, keyed by its ARN.
    pub projects: HashMap<String, DataAutomationProjectRecord>,
    /// `projectName` -> `projectArn`; the uniqueness index behind
    /// `CreateDataAutomationProject`'s `ConflictException`.
    pub project_names: HashMap<String, String>,
    /// `CreateDataAutomationProject` `clientToken` -> the ARN that call created, so
    /// an `@idempotencyToken` replay returns the original project.
    pub project_create_idempotency: HashMap<String, String>,
    /// Every stored DataAutomationLibrary, keyed by its ARN.
    pub libraries: HashMap<String, DataAutomationLibraryRecord>,
    /// `libraryName` -> `libraryArn`; the uniqueness index behind
    /// `CreateDataAutomationLibrary`'s `ConflictException`.
    pub library_names: HashMap<String, String>,
    /// `CreateDataAutomationLibrary` `clientToken` -> the ARN that call created, so
    /// an `@idempotencyToken` replay returns the original library.
    pub library_create_idempotency: HashMap<String, String>,
    /// Every data automation library ingestion job
    /// `InvokeDataAutomationLibraryIngestionJob` has accepted, keyed by the `jobArn` it
    /// minted for it, and read back by `GetDataAutomationLibraryIngestionJob` and
    /// `ListDataAutomationLibraryIngestionJobs`. Each record names the library it was
    /// submitted against, which is what scopes a listing to one library. Bounded at
    /// [`library_ingestion::INGESTION_JOB_QUOTA`] by the submission, which refuses rather
    /// than inserting past it.
    pub ingestion_jobs:
        HashMap<String, library_ingestion::DataAutomationLibraryIngestionJobRecord>,
    /// `InvokeDataAutomationLibraryIngestionJob` `clientToken` -> the `jobArn` that call
    /// minted, so an `@idempotencyToken` replay answers about the job the first call
    /// accepted rather than accepting a second one.
    ///
    /// Bounded by [`library_ingestion::INGESTION_JOB_QUOTA`] with the map above: an
    /// accepted submission inserts at most one token entry beside the one job it minted,
    /// and the quota is checked before either insert — so a caller submitting under fresh
    /// tokens cannot grow this map past the job count.
    pub ingestion_job_idempotency: HashMap<String, String>,
    /// Every blueprint-optimization job `InvokeBlueprintOptimizationAsync` has
    /// accepted, keyed by the `invocationArn` it minted for it, and read back by
    /// `GetBlueprintOptimizationStatus`. Bounded at [`OPTIMIZATION_JOB_QUOTA`] by the
    /// submission, which refuses rather than inserting past it.
    pub optimization_jobs: HashMap<String, BlueprintOptimizationJobRecord>,
    /// The sequence number the next minted resource ARN carries, for every
    /// resource this store mints one for (a blueprint, a DataAutomationProject). It
    /// counts the creates this store has served and is never rolled back, so every
    /// id it mints differs — two creates inside one nanosecond differ in the
    /// sequence, and deleting a record does not free its id. The whole 64-bit value
    /// goes into the id, so the counter saturates (at a count no in-memory store can
    /// reach) rather than wrapping onto an id already handed out.
    pub next_arn_sequence: u64,
}

impl ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState {
    /// Every stored blueprint in the listing order (`creationTime`, then ARN).
    pub fn blueprints_in_order(&self) -> Vec<&BlueprintRecord> {
        in_listing_order(self.blueprints.values())
    }

    /// Every stored DataAutomationProject in the listing order (`creationTime`, then
    /// ARN) — the order [`in_listing_order`] gives every listing of this service.
    pub fn projects_in_order(&self) -> Vec<&DataAutomationProjectRecord> {
        in_listing_order(self.projects.values())
    }

    /// Every stored DataAutomationLibrary in that same listing order.
    pub fn libraries_in_order(&self) -> Vec<&DataAutomationLibraryRecord> {
        in_listing_order(self.libraries.values())
    }

    /// Reserve the next blueprint-ARN sequence number, advancing the counter.
    pub fn take_arn_sequence(&mut self) -> u64 {
        let sequence = self.next_arn_sequence;
        self.next_arn_sequence = sequence.saturating_add(1);
        sequence
    }

    /// Forget a blueprint entirely: the record, its name reservation and any
    /// idempotency token that resolves to it.
    pub fn remove_blueprint(&mut self, arn: &str) -> Option<BlueprintRecord> {
        let record = self.blueprints.remove(arn)?;
        self.blueprint_names.remove(&record.blueprint_name);
        self.create_idempotency.retain(|_, stored| stored != arn);
        self.version_idempotency
            .retain(|_, (stored, _)| stored != arn);
        self.stage_copy_tokens
            .retain(|(stored, _, _), _| stored != arn);
        Some(record)
    }

    /// Forget the `CreateBlueprintVersion` client tokens that resolve to one
    /// snapshot, so a released snapshot releases its token with it: a token
    /// entry outlives nothing it points at.
    pub fn forget_version_token(&mut self, arn: &str, version: u64) {
        self.version_idempotency.retain(|_, (stored_arn, stored_version)| {
            stored_arn != arn || *stored_version != version
        });
    }

    /// Whether this store can tell that the submission's blueprint reference names NO
    /// blueprint of this account: either nothing is stored under `blueprint_arn`, or
    /// what is stored does not exist at the `stage` the submission selected — a
    /// reference a
    /// blueprint-optimization job cannot be run against, because the blueprint's schema
    /// IS what such a job optimizes.
    ///
    /// `false` for an AWS-owned (public) blueprint ARN: this store holds no catalog of
    /// those, so absence here is no evidence about them
    /// ([`names_account_owned_resource`]).
    pub fn names_unknown_blueprint(&self, blueprint_arn: &str, stage: Option<&str>) -> bool {
        if !names_account_owned_resource(blueprint_arn) {
            return false;
        }
        match self.blueprints.get(blueprint_arn) {
            None => true,
            Some(record) => stage.is_some_and(|stage| !record.exists_at(stage)),
        }
    }

    /// Forget a DataAutomationProject entirely: the record, its name reservation
    /// and any idempotency token that resolves to it. This is what keeps the store
    /// inside [`PROJECT_QUOTA`] over the life of the process — every record a
    /// create inserts is releasable through `DeleteDataAutomationProject`.
    pub fn remove_project(&mut self, arn: &str) -> Option<DataAutomationProjectRecord> {
        let record = self.projects.remove(arn)?;
        self.project_names.remove(&record.project_name);
        self.project_create_idempotency
            .retain(|_, stored| stored != arn);
        Some(record)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState>)
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries
/// no SigV4 credential scope. Only ARN minting needs it; every other operation
/// keys on the ARN the caller supplies.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// Mint a fresh blueprint ARN for `region`.
///
/// `smithy_query(shape="Blueprint")` constrains `blueprintArn` to
/// `^arn:aws(|-cn|-us-gov):bedrock:[a-zA-Z0-9-]*:(aws|[0-9]{12}):blueprint/(bedrock-data-automation-public-[a-zA-Z0-9-_]{1,30}|[a-zA-Z0-9-]{12,36})$`,
/// so the resource id is 12..36 characters of `[a-zA-Z0-9-]`. The id this mints
/// is 25 of them: the creating instant's nanoseconds-within-the-second, then the
/// store's sequence number in FULL 64-bit width. Uniqueness rests on that
/// sequence alone — it is never rolled back and never truncated here — so no two
/// records of one store ever share an id, whatever the clock does.
pub fn mint_blueprint_arn(
    region: &str,
    created_at: ::aws_smithy_types::DateTime,
    sequence: u64,
) -> String {
    let id = format!("{:08x}-{:016x}", created_at.subsec_nanos(), sequence);
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("blueprint/{id}"))
}

/// Mint a fresh DataAutomationProject ARN for `region`.
///
/// `smithy_query(shape="DataAutomationProject")` constrains `projectArn` to
/// `^arn:aws(|-cn|-us-gov):bedrock:[a-zA-Z0-9-]*:(aws|[0-9]{12}):data-automation-project/[a-zA-Z0-9-]{12,36}$`,
/// so the resource id is 12..36 characters of `[a-zA-Z0-9-]` — the same id space
/// [`mint_blueprint_arn`] mints into, under this service's other resource type, so
/// the two share one id format and one uniqueness argument (the store's sequence
/// number, never rolled back and never truncated).
pub fn mint_project_arn(
    region: &str,
    created_at: ::aws_smithy_types::DateTime,
    sequence: u64,
) -> String {
    let id = format!("{:08x}-{:016x}", created_at.subsec_nanos(), sequence);
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("data-automation-project/{id}"))
}

/// Mint a fresh DataAutomationLibrary ARN for `region`.
///
/// `smithy_query(shape="DataAutomationLibrary")` constrains `libraryArn` to
/// `^arn:aws(|-cn|-iso|-iso-[a-z]|-us-gov):bedrock:[a-zA-Z0-9-]*:[0-9]{12}:data-automation-library/[a-zA-Z0-9-]{12,36}$`
/// with `@length(..128)`, so the resource id is 12..36 characters of
/// `[a-zA-Z0-9-]` — the same id space [`mint_blueprint_arn`] mints into, under this
/// service's library resource type, so the three share one id format and one
/// uniqueness argument (the store's sequence number, never rolled back and never
/// truncated). Unlike the project pattern this one admits no `aws` account field, and
/// `region` is reduced to a region label through the shared
/// `service_common::arn::region_label` grammar — non-empty, at most 20 characters,
/// only `[a-z0-9-]` — so the whole value stays inside the pattern and well under 128
/// characters whatever a caller signs for.
pub fn mint_data_automation_library_arn(
    region: &str,
    created_at: ::aws_smithy_types::DateTime,
    sequence: u64,
) -> String {
    let id = format!("{:08x}-{:016x}", created_at.subsec_nanos(), sequence);
    ::engine::emulator_arn(
        ARN_SERVICE,
        ::service_common::arn::region_label(region).unwrap_or(DEFAULT_REGION),
        &format!("data-automation-library/{id}"),
    )
}

/// Mint a fresh blueprint-optimization invocation ARN for `region`.
///
/// `smithy_query(operation="GetBlueprintOptimizationStatus")` constrains
/// `invocationArn` to
/// `^arn:aws(|-cn|-iso|-iso-[a-z]|-us-gov):bedrock:[a-zA-Z0-9-]*:[0-9]{12}:blueprint-optimization-invocation/[a-zA-Z0-9-_]+$`
/// with `@length(1..128)`, and `InvokeBlueprintOptimizationAsync` returns the value in
/// a member carrying the same constraints — so an ARN this service mints has to satisfy
/// them, or it would name a job no caller could ever address. The resource id is the
/// one [`mint_blueprint_arn`] builds and rests its uniqueness on (the creating instant's
/// nanoseconds-within-the-second, then the store's never-rolled-back sequence number in
/// full 64-bit width), and `region` is reduced to a region label through the shared
/// `service_common::arn::region_label` grammar — non-empty, at most 20 characters, only
/// `[a-z0-9-]`, which is the form the pattern's region field states — so the whole value
/// stays inside the pattern and well under 128 characters whatever a caller signs for.
pub fn mint_blueprint_optimization_invocation_arn(
    region: &str,
    created_at: ::aws_smithy_types::DateTime,
    sequence: u64,
) -> String {
    let id = format!("{:08x}-{:016x}", created_at.subsec_nanos(), sequence);
    ::engine::emulator_arn(
        ARN_SERVICE,
        ::service_common::arn::region_label(region).unwrap_or(DEFAULT_REGION),
        &format!("{OPTIMIZATION_INVOCATION_RESOURCE_PREFIX}{id}"),
    )
}

/// The current wall-clock instant as a modeled `date-time` timestamp.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now())
}

/// The current instant in BOTH forms one call needs: the `SystemTime` a stored due
/// instant is computed from and compared against, and the same instant as the
/// modeled `date-time` timestamp a response carries. Read once, so a record's
/// `creationTime` and its due instant cannot come from two different clock reads.
pub fn now_pair() -> (SystemTime, ::aws_smithy_types::DateTime) {
    let at = SystemTime::now();
    (at, ::aws_smithy_types::DateTime::from(at))
}

/// A record this service's paginated listings order and page over: a blueprint, a
/// DataAutomationProject. One order and one cursor for every listing, so two
/// listings cannot disagree about where a page boundary falls.
pub trait ListedResource {
    /// The record's stable sort key: creation time, then ARN. Listings order by
    /// this so a page boundary is reproducible across calls.
    fn order_key(&self) -> (i64, u32, &str);
}

impl ListedResource for BlueprintRecord {
    fn order_key(&self) -> (i64, u32, &str) {
        (
            self.creation_time.secs(),
            self.creation_time.subsec_nanos(),
            self.blueprint_arn.as_str(),
        )
    }
}

impl ListedResource for DataAutomationProjectRecord {
    fn order_key(&self) -> (i64, u32, &str) {
        (
            self.creation_time.secs(),
            self.creation_time.subsec_nanos(),
            self.project_arn.as_str(),
        )
    }
}

impl ListedResource for DataAutomationLibraryRecord {
    fn order_key(&self) -> (i64, u32, &str) {
        (
            self.creation_time.secs(),
            self.creation_time.subsec_nanos(),
            self.library_arn.as_str(),
        )
    }
}

/// Sort the records of one store map into the listing order.
pub fn in_listing_order<'a, R: ListedResource + 'a>(
    records: impl Iterator<Item = &'a R>,
) -> Vec<&'a R> {
    let mut records: Vec<&R> = records.collect();
    records.sort_by(|a, b| a.order_key().cmp(&b.order_key()));
    records
}

/// A page cursor for one of this service's listings: the sort key of the last item
/// the previous page returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor {
    pub secs: i64,
    pub subsec_nanos: u32,
    pub resource_arn: String,
}

impl PageCursor {
    /// The cursor just after `record`.
    pub fn after<R: ListedResource>(record: &R) -> Self {
        let (secs, subsec_nanos, resource_arn) = record.order_key();
        Self {
            secs,
            subsec_nanos,
            resource_arn: resource_arn.to_string(),
        }
    }

    /// Is `record` strictly after this cursor in the listing order?
    pub fn precedes<R: ListedResource>(&self, record: &R) -> bool {
        record.order_key() > (self.secs, self.subsec_nanos, self.resource_arn.as_str())
    }

    /// Render as an opaque token. Base64 (the codec the wire protocols already
    /// use for blobs, `aws_smithy_types::base64`) keeps the token inside
    /// `NextToken`'s modeled `^\S*$` pattern and 1..2048 length whatever the ARN
    /// contains.
    ///
    /// The token carries no integrity tag: this emulator's listings are a single
    /// in-process store, the decoded cursor is only compared ordinally in
    /// [`Self::precedes`] and is never used to look up a record, so a client that
    /// edits one can only choose where its OWN listing resumes. Real AWS tokens
    /// are integrity-protected, so a test asserting token opacity will see a
    /// difference here.
    pub fn to_token(&self) -> String {
        ::aws_smithy_types::base64::encode(format!(
            "{}:{}:{}",
            self.secs, self.subsec_nanos, self.resource_arn
        ))
    }

    /// Parse a token this service minted. `None` for anything else — the caller
    /// answers that with the modeled `ValidationException`.
    pub fn from_token(token: &str) -> Option<Self> {
        let decoded =
            String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
        let mut fields = decoded.splitn(3, ':');
        let secs = fields.next()?.parse().ok()?;
        let subsec_nanos = fields.next()?.parse().ok()?;
        let resource_arn = fields.next()?;
        if resource_arn.is_empty() {
            return None;
        }
        Some(Self {
            secs,
            subsec_nanos,
            resource_arn: resource_arn.to_string(),
        })
    }
}
