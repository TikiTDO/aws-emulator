// SPDX-License-Identifier: CC0-1.0

//! The FILES a recommendations export job writes, and the write itself.
//!
//! Every `Export*Recommendations` operation of this service queues one asynchronous export:
//! "Recommendations are exported in a comma-separated values (.csv) file, and its metadata
//! in a JavaScript Object Notation (JSON) (.json) file, to an existing Amazon Simple Storage
//! Service (Amazon S3) bucket that you specify." The job record and its lifecycle live in
//! [`super`]; this module is the other half — WHAT the two objects hold, and the write that
//! puts them in the destination bucket once the job completes.
//!
//! The one object store in this emulator is the Amazon S3 service's own, so the export
//! objects are written there and are then read back with `GetObject` / `ListObjectsV2` like
//! any other object. That is also why a destination this emulator does not hold is REFUSED
//! at the export call ([`destination_bucket_rejection`]): "You must create the destination
//! Amazon S3 bucket for your recommendations export before you create the export job.
//! Compute Optimizer does not create the S3 bucket for you."
//!
//! What the files hold:
//!
//! * the export file — one HEADER row of the `Exportable*Field` names the request asked for
//!   ("The recommendations data to include in the export file"), then one row per
//!   recommendation the account's analysis holds for the job's resource type, and one per
//!   recommendation OPTION where the fields name an option's values (each
//!   `RecommendationOptions*` field names ONE option). Each cell is that field's value for
//!   that recommendation, computed from the stored analysis the same way the recommendation
//!   READER computes it, so an exported row and a `Get*Recommendations` response cannot
//!   disagree. A field the stored analysis records no value for is an empty cell, and so is one
//!   that names no member of the surface's recommendation at all. Where a surface's
//!   recommendation carries members the shared findings-catalog row does not — the Amazon EC2
//!   instance surface, whose options carry an instance type, and the Aurora and RDS database
//!   surface, whose catalog row records the database's own configuration — the row is built from
//!   the observation the reader reports the recommendation from rather than from that catalog
//!   row, so the one rule serves both.
//! * the metadata file — what the export job itself was: its id, the resource type and
//!   account(s) it covers, the recommendation preferences it was asked for, the file format and
//!   columns of the export file, how many rows it holds and where both objects were written.
//!
//! AWS publishes no column layout, row order or metadata schema for either file, so both are
//! the emulator's own; every value in them is nevertheless derived from the stored analysis
//! and from the request that asked for the export.
//!
//! ## The clock
//!
//! The objects appear when the job REACHES `Complete` ([`ExportJob::status_at`]), not when
//! the export call returns: a client that reads the destination while the job is still
//! `Queued` or `InProgress` finds nothing there, as it would against the real service. The
//! files are computed at the export call (so the export is of the analysis as it stood when
//! the job was created) and held with the job until the clock makes the completion due;
//! [`write_due_exports`] then performs exactly the writes the stored due instant has already
//! made due, and no operation ever sleeps for it. A client that polls
//! `DescribeRecommendationExportJobs` to `Complete` and then downloads the file is the
//! documented workflow, and [`drive_export_clock`] starts one short in-process task per job
//! so the objects also appear for a client that never calls this API again.
//!
//! ## Lock order
//!
//! This service's store is locked, the due objects are TAKEN out of it, and the lock is
//! released before the object store is locked: the two locks are never held at once, and the
//! Amazon S3 service never reaches into this one, so the two cannot deadlock.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use ::service_com_amazonaws_s3_amazons3 as s3;

use super::{
    architecture_admitted, external_metric_status, look_back_period_in_days, parse_resource_arn,
    percentage_of, round_hundredths, stated_architectures, AnalysedResource, AnalyzedAutoScalingGroup,
    ComAmazonawsComputeoptimizerComputeoptimizerserviceState, EffectivePreferences, ExportJob,
    GroupConfiguration, IdleResourceAnalysis, InstanceConfiguration, ObservedInstance,
    ObservedLicensedInstance, CURRENCY_USD, IDLE_FALSE, IDLE_TRUE,
    RESOURCE_TYPE_AUTO_SCALING_GROUP, RESOURCE_TYPE_EBS_VOLUME, RESOURCE_TYPE_EC2_INSTANCE,
    RESOURCE_TYPE_ECS_SERVICE, RESOURCE_TYPE_IDLE, RESOURCE_TYPE_LAMBDA_FUNCTION,
    RESOURCE_TYPE_LICENSE, RESOURCE_TYPE_RDS_DB_INSTANCE, SAVINGS_ESTIMATION_SOURCE,
};
use crate::ops::types::{
    DBStorageConfiguration, RDSDBRecommendation, RDSDBUtilizationMetric, SavingsOpportunity,
    EXPORTABLE_AUTO_SCALING_GROUP_FIELD_VALUES, EXPORTABLE_IDLE_FIELD_VALUES,
    EXPORTABLE_INSTANCE_FIELD_VALUES, EXPORTABLE_R_D_S_D_B_FIELD_VALUES,
};

/// The one `FileFormat` the model declares: "The only export file format currently supported
/// is Csv" (`crate::ops::types::FILE_FORMAT_VALUES`).
pub const FILE_FORMAT_CSV: &str = "Csv";

/// How a cell holding several values separates them, so one exportable field is still one
/// column. AWS publishes no layout for the export file, so this is the emulator's own.
const LIST_SEPARATOR: &str = "|";

/// One row of an export file: the value of each exportable field the recommendation carries,
/// keyed by the field's modelled name (an `Exportable*Field` value). A field the analysis
/// records no value for has NO entry, and the cell under its column is empty.
pub type ExportRow = BTreeMap<String, String>;

/// One filter of any of this service's filter vocabularies.
///
/// Each `Export*Recommendations` operation carries its own filter shape and every one of them
/// is a name with the values it selects, so this is what lets one [`ExportPlan`] take the
/// filters of any of those surfaces ([`filter_pairs`]) instead of each handler restating the
/// same mapping.
pub trait ExportFilter {
    /// The `*FilterName` the filter carries, or `None` for one that names nothing.
    fn filter_name(&self) -> Option<&str>;
    /// The values it selects on; empty constrains nothing.
    fn filter_values(&self) -> &[String];
}

/// Implement [`ExportFilter`] for the modelled filter shapes, which all declare the same two
/// members.
macro_rules! export_filters {
    ($($shape:ty),+ $(,)?) => {
        $(impl ExportFilter for $shape {
            fn filter_name(&self) -> Option<&str> {
                self.name.as_deref()
            }

            fn filter_values(&self) -> &[String] {
                self.values.as_deref().unwrap_or_default()
            }
        })+
    };
}

export_filters!(
    crate::ops::types::Filter,
    crate::ops::types::EBSFilter,
    crate::ops::types::ECSServiceRecommendationFilter,
    crate::ops::types::IdleRecommendationFilter,
    crate::ops::types::LambdaFunctionRecommendationFilter,
    crate::ops::types::LicenseRecommendationFilter,
    crate::ops::types::RDSDBRecommendationFilter,
);

/// A request's filters as the (name, values) pairs an [`ExportPlan`] carries.
pub fn filter_pairs<F: ExportFilter>(filters: &[F]) -> Vec<(String, Vec<String>)> {
    filters
        .iter()
        .map(|filter| {
            (
                filter.filter_name().unwrap_or_default().to_string(),
                filter.filter_values().to_vec(),
            )
        })
        .collect()
}

/// Everything one export call settles about the job it creates: where the two objects go, and
/// what they hold. The job record and both files are derived from this, so the destination the
/// call answers, the destination the job reports and the objects that actually appear cannot
/// name different places.
#[derive(Debug, Clone)]
pub struct ExportPlan {
    /// The `JobId` minted for the job.
    pub job_id: String,
    /// The Region whose endpoint created it.
    pub region: String,
    /// The `ResourceType` whose recommendations are exported.
    pub resource_type: String,
    /// The destination bucket the request named, and the two object keys the export file and
    /// its metadata file are written under ([`super::export_destination_keys`]).
    pub bucket: String,
    /// See [`ExportPlan::bucket`].
    pub key: String,
    /// See [`ExportPlan::bucket`].
    pub metadata_key: String,
    /// The columns of the export file ([`export_columns`]).
    pub columns: Vec<String>,
    /// The `FileFormat` the export file is written in.
    pub file_format: String,
    /// The `accountIds` the request named, and whether it asked for the organization's member
    /// accounts — what the export covers, which the metadata file states.
    pub account_ids: Vec<String>,
    /// See [`ExportPlan::account_ids`].
    pub include_member_accounts: bool,
    /// The request's filters as (name, values) pairs — "an array of objects to specify a
    /// filter that exports a more specific set of ... recommendations" — which select which
    /// recommendations the export file holds ([`row_matches_filters`]).
    pub filters: Vec<(String, Vec<String>)>,
    /// The `cpuVendorArchitectures` of the `recommendationPreferences` the request named: the
    /// architectures the exported recommendations are generated under, which select which
    /// recommendation options the export file holds and which architectures its rows state.
    /// Empty for a request that asked for none — and for a surface whose model declares no
    /// such member — which restricts nothing.
    pub cpu_vendor_architectures: Vec<String>,
    /// The Amazon Aurora and RDS databases the account held an analysis for when the export
    /// was created ([`super::analysed_databases`]): the DB instances of its own Amazon RDS
    /// inventory, beside the database rows of the findings catalog. They are resolved with the
    /// plan — "the export is of the recommendations the account held when it asked for the
    /// export" — so an exported file holds exactly the databases
    /// `GetRDSDatabaseRecommendations` reported at that moment. Empty for every other resource
    /// type's export, which no database row belongs to.
    pub analysed_databases: Vec<AnalysedResource>,
}

/// The two objects an export job has computed but not yet written: the export file and its
/// metadata file, held with the job until the clock makes the job's completion due.
#[derive(Debug, Clone)]
pub struct PendingExport {
    /// The export file's bytes.
    pub export_file: String,
    /// The metadata file's bytes.
    pub metadata_file: String,
}

/// One job's objects, taken out of the store because the clock has made the job's completion
/// due, with where they go — what [`write_due_exports`] then writes.
#[derive(Debug, Clone)]
pub struct DueExport {
    /// The job the objects belong to, which is what a failed write is recorded against.
    pub job_id: String,
    /// The destination bucket, and the keys the two objects are written under.
    pub bucket: String,
    /// See [`DueExport::bucket`].
    pub key: String,
    /// See [`DueExport::bucket`].
    pub metadata_key: String,
    /// The objects' bytes.
    pub pending: PendingExport,
}

/// Why a request's destination is not one this export can be written to, or `None` when it
/// is.
///
/// "You must create the destination Amazon S3 bucket for your recommendations export before
/// you create the export job. Compute Optimizer does not create the S3 bucket for you" — so a
/// request naming a bucket the object store does not hold is refused rather than accepted
/// into a job whose file could never appear.
///
/// The refusal is the service's own constant wording for the condition
/// ([`MISSING_DESTINATION_MESSAGE`]): it states that the destination does not exist and what to
/// do about it, and names neither the bucket nor the member — so no export operation words
/// this refusal differently.
pub fn destination_bucket_rejection<Eng: Send + Sync + 'static>(
    engine: &Eng,
    bucket: &str,
) -> Option<String> {
    let held = s3::state::service_state(engine)
        .is_some_and(|store| store.lock().bucket(bucket).is_some());
    (!held).then(|| MISSING_DESTINATION_MESSAGE.to_string())
}

/// What every export operation answers when the destination it was given is not a bucket this
/// emulator's object store holds. ONE spelling for the whole crate, so the eight exports
/// cannot drift apart on the same refusal.
pub const MISSING_DESTINATION_MESSAGE: &str =
    "The Amazon S3 destination provided does not exist. Confirm your S3 destination and try \
     again.";

/// What every export operation answers a request made while one of its own resource type is
/// still in flight: the service's own constant sentence for the condition. It states the limit
/// and nothing else — neither the resource type, nor the Region, nor the job that still holds
/// the slot — so the refusal is the same text for every export surface and every run, and ONE
/// spelling serves the whole crate.
pub const EXPORT_JOB_LIMIT_MESSAGE: &str = "Export job limit exceeded. You can only have one \
     active job per resource type at a time.";

/// The columns of an export file: the fields the request asked for, in the order it asked for
/// them, or — for a request that names none — every field the resource type's
/// `Exportable*Field` enum declares.
///
/// "The recommendations data to include in the export file": the request chooses which of its
/// resource type's fields the file holds, and a request that chooses none gets the whole set
/// rather than a file of no columns.
///
/// Only the fields that resource type's enum DECLARES, each once: the column count is then
/// bounded by the size of that enum, so neither the file this builds nor the work of building
/// it can be grown by a request that repeats a name or carries a long list of them. (The
/// request boundary already refuses a value the enum does not declare, so a name dropped here
/// is one that could not have arrived.)
pub fn export_columns(requested: Option<&[String]>, modelled: &[&str]) -> Vec<String> {
    let requested = requested.unwrap_or_default();
    if requested.is_empty() {
        return modelled.iter().map(|field| (*field).to_string()).collect();
    }
    let declared: HashSet<&str> = modelled.iter().copied().collect();
    let mut taken: HashSet<&str> = HashSet::with_capacity(modelled.len());
    let mut columns: Vec<String> = Vec::new();
    for field in requested {
        if let Some(field) = declared.get(field.as_str()) {
            if taken.insert(field) {
                columns.push((*field).to_string());
            }
        }
    }
    columns
}

// ---------------------------------------------------------------------------
// Admitting one export call and creating its job
//
// Every `Export*Recommendations` operation of this service admits a request by the SAME
// rules — the mutually exclusive `accountIds`/`includeMemberAccounts` pair, the account ids
// themselves, the organization an `includeMemberAccounts` asks to cover, the surface's own
// filter vocabulary, a destination bucket that is named and that the object store holds, the
// enrollment gate, the one-job-in-progress-per-Region limit and how many jobs this store may
// hold — and then records the same job against the same derived keys. So that whole rule lives HERE once, and each export operation supplies only
// what its own surface settles: the resource type it exports, its filter vocabulary, its
// `Exportable*Field` set and the sentence it documents the one-job limit with.
// ---------------------------------------------------------------------------

/// What one `Export*Recommendations` request asks for, in the terms the shared admission rule
/// needs: the members every export surface declares, plus the few things that differ between
/// surfaces.
pub struct ExportRequest<'a, F: ExportFilter> {
    /// The `ResourceType` whose recommendations this surface exports, which the job carries
    /// and which the one-job-in-progress limit is counted per.
    pub resource_type: &'a str,
    /// The request's `accountIds` and whether it asked for the organization's member accounts
    /// — "The parameters are mutually exclusive." They select WHOSE recommendations go into
    /// the file, not where it is written, so an id naming an account this service cannot
    /// answer about ([`super::is_foreign_account`]) is refused rather than exported under.
    pub account_ids: Option<Vec<String>>,
    /// See [`ExportRequest::account_ids`]. Asking for the member accounts is asking for the
    /// resources of "all member accounts of the organization", which only a management account
    /// or delegated administrator has, so a caller that manages none is refused
    /// ([`super::ComAmazonawsComputeoptimizerComputeoptimizerserviceState::include_member_accounts_rejection`]).
    pub include_member_accounts: bool,
    /// The request's filters, in this surface's own filter shape: "an array of objects to
    /// specify a filter that exports a more specific set of ... recommendations".
    pub filters: Option<Vec<F>>,
    /// This surface's filter vocabulary check — the one its recommendation READER applies, so
    /// an export and a read never admit different filter sets.
    pub filter_rejection: fn(&[F]) -> Option<String>,
    /// The `s3DestinationConfig` the request named: the destination bucket and the key prefix
    /// the two object keys are derived from.
    pub destination: &'a crate::ops::types::S3DestinationConfig,
    /// "The recommendations data to include in the export file", with this surface's
    /// `Exportable*Field` value set to resolve it against ([`export_columns`]).
    pub fields_to_export: Option<&'a [String]>,
    /// See [`ExportRequest::fields_to_export`].
    pub exportable_fields: &'a [&'a str],
    /// The `fileFormat` the request asked for, or `None` for the one format the model
    /// declares ([`FILE_FORMAT_CSV`]).
    pub file_format: Option<String>,
    /// The `recommendationPreferences` the request carries, for the export surfaces whose
    /// model declares that member, and `None` for the surfaces that declare none. It
    /// "describes the recommendation preferences to return in the response of a
    /// GetAutoScalingGroupRecommendations, GetEC2InstanceRecommendations … and
    /// GetRDSDatabaseRecommendations request", so an EXPORT of those same recommendations is
    /// of the recommendations the preferences settle — resolved here by the rule the surface's
    /// own READER resolves it by, so the file and the response cannot disagree.
    pub recommendation_preferences: Option<crate::ops::types::RecommendationPreferences>,
}

/// Why one export call cannot create its job, as the modelled error the calling operation
/// answers it with: the shared rule names the refusal and each operation lifts it into its own
/// error channel.
#[derive(Debug, Clone)]
pub enum ExportRefusal {
    /// This service's store is not reachable from the engine — an `InternalServerException`.
    StoreUnavailable,
    /// The request is not one the service accepts — an `InvalidParameterValueException`.
    InvalidParameter(String),
    /// The account is not enrolled — an `OptInRequiredException`.
    OptInRequired,
    /// The call would exceed a limit of the service — a `LimitExceededException`.
    LimitExceeded(String),
}

impl ExportRefusal {
    /// The message the refusal carries, stated here so no two export operations word one
    /// refusal differently.
    pub fn message(&self) -> String {
        match self {
            Self::StoreUnavailable => {
                "The Compute Optimizer recommendation store is unavailable.".to_string()
            }
            Self::OptInRequired => {
                "The account is not opted in to Compute Optimizer.".to_string()
            }
            Self::InvalidParameter(message) | Self::LimitExceeded(message) => message.clone(),
        }
    }
}

/// What an admitted export call answers: the job's id, and the destination the job itself
/// holds — so the call's answer, the job's reported destination and the objects that appear
/// cannot name different places.
#[derive(Debug, Clone)]
pub struct CreatedExport {
    /// The `JobId` minted for the created job.
    pub job_id: String,
    /// The `S3Destination` of the created job.
    pub s3_destination: crate::ops::types::S3Destination,
}

/// Admit one `Export*Recommendations` request and create its job: the whole rule every export
/// operation of this service applies.
///
/// A malformed request is refused BEFORE the store is read, so an invalid parameter takes
/// precedence over the enrollment gate, as it does in every other operation of this service.
/// An `accountIds` naming another account is such a request: it asks for an export of an
/// account this service answers nothing about, and is refused with the crate's one
/// cross-account refusal ([`super::CROSS_ACCOUNT_MESSAGE`]) exactly as a reader refuses it. So
/// is an `includeMemberAccounts` asking to cover the organization's member accounts from a
/// caller that manages no organization, which is refused before the enrollment gate and
/// before any job is recorded.
/// Creating a job is a write, so this is also where the jobs the seven-day reporting window
/// has dropped are let go of — a reader never writes. The export job belongs to the Region
/// whose endpoint created it, and so does the one-job-in-progress limit each export operation
/// documents: "You can have only one ... export job in progress per Amazon Web Services
/// Region", which a job that has reached its terminal status no longer holds, and which a job
/// of another resource type never holds against this one. That refusal carries the service's
/// one constant sentence for the condition ([`EXPORT_JOB_LIMIT_MESSAGE`]), whatever the
/// resource type and whichever job holds the slot.
pub fn create_export_job<Eng: Send + Sync + 'static, F: ExportFilter>(
    engine: &Arc<Eng>,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    request: ExportRequest<'_, F>,
) -> Result<CreatedExport, ExportRefusal> {
    let Some(state) = super::service_state(&**engine) else {
        return Err(ExportRefusal::StoreUnavailable);
    };

    // `accountIds`: "You can specify multiple account IDs per request", each an Amazon Web
    // Services account ID. "This parameter cannot be specified together with the include
    // member accounts parameter. The parameters are mutually exclusive."
    let account_ids = request.account_ids.unwrap_or_default();
    if !account_ids.is_empty() && request.include_member_accounts {
        return Err(ExportRefusal::InvalidParameter(
            "The value supplied for parameter accountIds is not valid: accountIds and \
             includeMemberAccounts are mutually exclusive."
                .to_string(),
        ));
    }
    if let Some(value) = account_ids
        .iter()
        .find(|value| !super::is_account_id(value))
    {
        return Err(ExportRefusal::InvalidParameter(format!(
            "The value supplied for parameter accountIds is not valid: {value} is not a \
             12-digit Amazon Web Services account ID."
        )));
    }
    // `accountIds` is the MANAGEMENT-account mechanism for naming a member account of an
    // organization, and this emulator presents exactly one standalone account
    // ([`super::is_foreign_account`]) and records no organization. So an id naming any OTHER
    // account asks for an export of an account whose analysis this service does not hold, and
    // is refused with the refusal the whole crate shares ([`super::CROSS_ACCOUNT_MESSAGE`]) —
    // the same answer a `Get*Recommendations` reader gives the same id, rather than writing
    // THIS account's rows into a file whose metadata names that other account.
    if account_ids
        .iter()
        .any(|value| super::is_foreign_account(value))
    {
        return Err(ExportRefusal::InvalidParameter(
            super::CROSS_ACCOUNT_MESSAGE.to_string(),
        ));
    }

    // The filters are the ones the surface's recommendation reader selects with, so a name or
    // value outside that vocabulary is refused here too rather than silently exporting a
    // different set.
    let filters = request.filters.unwrap_or_default();
    if let Some(reason) = (request.filter_rejection)(&filters) {
        return Err(ExportRefusal::InvalidParameter(reason));
    }

    // "You must create the destination Amazon S3 bucket for your recommendations export
    // before you create the export job. Compute Optimizer does not create the S3 bucket for
    // you." There is no export without a bucket to write it to, so a request that names none
    // is refused — and so is one naming a bucket this emulator's object store does not hold,
    // rather than queueing a job whose export file could never appear.
    let bucket = request
        .destination
        .bucket
        .as_deref()
        .map(str::trim)
        .filter(|bucket| !bucket.is_empty());
    let Some(bucket) = bucket else {
        return Err(ExportRefusal::InvalidParameter(
            "The value supplied for parameter s3DestinationConfig is not valid: a \
             destination bucket must be supplied."
                .to_string(),
        ));
    };
    if let Some(reason) = destination_bucket_rejection(&**engine, bucket) {
        return Err(ExportRefusal::InvalidParameter(reason));
    }

    let region = super::request_region(ctx);
    let now = super::now_secs();

    let mut guard = state.lock();
    // `includeMemberAccounts` asks for "recommendations for resources in all member accounts
    // of the organization", which only an organization's management account or delegated
    // administrator has, so a caller that manages none is refused rather than handed a job
    // claiming to cover an organization this service records nothing about
    // ([`super::include_member_accounts_rejection`]). Refused BEFORE the enrollment gate: it
    // is about what the request asks for, not about what the store holds for the caller, as
    // every other invalid parameter of this rule is.
    if let Some(message) = guard.include_member_accounts_rejection(request.include_member_accounts)
    {
        return Err(ExportRefusal::InvalidParameter(message.to_string()));
    }
    if !guard.enrollment.is_opted_in() {
        return Err(ExportRefusal::OptInRequired);
    }

    guard.export_jobs.prune_expired(now);

    if guard
        .export_jobs
        .in_progress_in(&region, request.resource_type, now)
        .is_some()
    {
        return Err(ExportRefusal::LimitExceeded(
            EXPORT_JOB_LIMIT_MESSAGE.to_string(),
        ));
    }
    if !guard.export_jobs.can_hold() {
        return Err(ExportRefusal::LimitExceeded(format!(
            "The request exceeds a limit of the service: this emulator holds at most {} \
             recommendation export jobs.",
            super::MAX_EXPORT_JOBS
        )));
    }

    // `recommendationPreferences$cpuVendorArchitectures` is a list of the modelled
    // `CpuVendorArchitecture` enum, so the request boundary has already refused any value
    // outside it and what reaches here is a list of architectures — the ones the exported
    // recommendations are generated under.
    let cpu_vendor_architectures = request
        .recommendation_preferences
        .and_then(|preferences| preferences.cpu_vendor_architectures)
        .unwrap_or_default();

    let job_id = guard.export_jobs.mint_job_id();
    let (key, metadata_key) = super::export_destination_keys(
        request.destination.key_prefix.as_deref(),
        ::engine::emulator_account_id(),
        request.resource_type,
        &job_id,
    );
    let plan = ExportPlan {
        job_id: job_id.clone(),
        region: region.clone(),
        resource_type: request.resource_type.to_string(),
        bucket: bucket.to_string(),
        key,
        metadata_key,
        // "The recommendations data to include in the export file", or every field the model
        // declares for this resource type when the request selects none.
        columns: export_columns(request.fields_to_export, request.exportable_fields),
        file_format: request
            .file_format
            .unwrap_or_else(|| FILE_FORMAT_CSV.to_string()),
        account_ids,
        include_member_accounts: request.include_member_accounts,
        filters: filter_pairs(&filters),
        cpu_vendor_architectures,
        // The databases the account holds an analysis for, resolved through the ONE rule every
        // database surface of this service reads them by, so an export and a read of the same
        // moment cannot differ on which databases the account has.
        analysed_databases: if request.resource_type == RESOURCE_TYPE_RDS_DB_INSTANCE {
            super::analysed_databases(
                &**engine,
                &guard,
                &region,
                ::engine::emulator_account_id(),
                now,
            )
        } else {
            Vec::new()
        },
    };
    // The destination this call answers is the job's own, so it names exactly the objects
    // `DescribeRecommendationExportJobs` reports for the job and exactly the objects the
    // completed job writes.
    let s3_destination = guard.record_export_job(&plan, now).s3_destination();
    drop(guard);

    // An export this call did not create may have come due meanwhile, and this one comes due
    // two lifecycle steps from now — the task is what makes the objects appear for a client
    // that makes no further call to this service.
    deliver_and_schedule(engine, now);

    Ok(CreatedExport {
        job_id,
        s3_destination,
    })
}

/// The export file: the column header row, then one row per recommendation.
///
/// Comma-separated values as the `csv` crate writes them — quoting a cell that needs it, with
/// the CRLF record terminator RFC 4180 states — so the quoting rules are the codec's and not
/// re-derived here. A row holds no byte the codec can refuse, so the writer cannot fail; an
/// unexpected failure yields an EMPTY file rather than a truncated one a client could not tell
/// from a complete export, and is logged.
pub fn export_csv(columns: &[String], rows: &[ExportRow]) -> String {
    let mut writer = ::csv::WriterBuilder::new()
        .terminator(::csv::Terminator::CRLF)
        .from_writer(Vec::new());
    let mut write = || -> Result<(), ::csv::Error> {
        writer.write_record(columns)?;
        for row in rows {
            writer.write_record(
                columns
                    .iter()
                    .map(|column| row.get(column).map_or("", String::as_str)),
            )?;
        }
        writer.flush().map_err(::csv::Error::from)
    };
    if let Err(error) = write() {
        ::tracing::warn!(%error, "the recommendations export file could not be written");
        return String::new();
    }
    writer
        .into_inner()
        .map(|file| String::from_utf8_lossy(&file).into_owned())
        .unwrap_or_default()
}

/// The metadata file of the export `plan` describes, holding `rows` recommendation rows and
/// created at `created_at`: what the export job was, and where its two objects were written.
fn export_metadata(plan: &ExportPlan, rows: usize, created_at: f64) -> String {
    let mut metadata = ::serde_json::json!({
        "jobId": plan.job_id,
        "region": plan.region,
        "resourceType": plan.resource_type,
        "accountIds": if plan.account_ids.is_empty() {
            vec![::engine::emulator_account_id().to_string()]
        } else {
            plan.account_ids.clone()
        },
        "includeMemberAccounts": plan.include_member_accounts,
        "creationTimestamp": timestamp(created_at),
        "exportFile": {
            "fileFormat": plan.file_format,
            "columns": plan.columns,
            "rowCount": rows,
        },
        "destination": {
            "bucket": plan.bucket,
            "key": plan.key,
            "metadataKey": plan.metadata_key,
        },
    });
    // The recommendation preferences the export was asked for, stated only where the request
    // named any: a surface whose model declares no such member, and a request that asked for
    // no architecture, states no preference rather than an empty one.
    if !plan.cpu_vendor_architectures.is_empty() {
        metadata["recommendationPreferences"] = ::serde_json::json!({
            "cpuVendorArchitectures": plan.cpu_vendor_architectures,
        });
    }
    ::serde_json::to_string_pretty(&metadata).unwrap_or_else(|_| metadata.to_string())
}

/// The export COLUMN one filter name selects on, for the surface that exports
/// `resource_type` — the member the recommendation reader of that surface resolves the same
/// name to.
///
/// Each `Export*Recommendations` operation carries its own filter vocabulary, and a name is
/// spelled for the member it selects on rather than for the exportable field, so the two are
/// mapped here explicitly: the ECS, Lambda and License surfaces name one reason code
/// (`FindingReasonCode`) while the field is the list `FindingReasonCodes`, the License surface
/// selects on the license's name, and the database surface selects on the instance and storage
/// findings, their reason codes and whether the database is idle. `None` is a name the surface
/// resolves to no member of its own recommendation, which therefore selects nothing — the rule
/// the readers apply (`FindingReasonCodes` on the Auto Scaling group surface, say, whose
/// recommendation declares no such member).
fn filter_column_of(resource_type: &str, name: &str) -> Option<&'static str> {
    match (resource_type, name) {
        // The `Filters` vocabulary, shared by the Auto Scaling group and instance surfaces.
        (_, "Finding") => Some("Finding"),
        (_, "InferredWorkloadTypes") => Some("InferredWorkloadTypes"),
        // `FindingReasonCodes` selects nothing on the Auto Scaling group surface: an
        // `AutoScalingGroupRecommendation` declares no finding reason codes.
        (RESOURCE_TYPE_AUTO_SCALING_GROUP, "FindingReasonCodes") => None,
        (_, "FindingReasonCodes" | "FindingReasonCode") => Some("FindingReasonCodes"),
        // The idle resource surface.
        (RESOURCE_TYPE_IDLE, "ResourceType") => Some("ResourceType"),
        // The commercial software license surface.
        (RESOURCE_TYPE_LICENSE, "LicenseName") => {
            Some("CurrentLicenseConfigurationLicenseName")
        }
        // The database surface, which reports a finding per side of the analysis.
        (RESOURCE_TYPE_RDS_DB_INSTANCE, "InstanceFinding") => Some("InstanceFinding"),
        (RESOURCE_TYPE_RDS_DB_INSTANCE, "StorageFinding") => Some("StorageFinding"),
        (RESOURCE_TYPE_RDS_DB_INSTANCE, "InstanceFindingReasonCode") => {
            Some("InstanceFindingReasonCodes")
        }
        (RESOURCE_TYPE_RDS_DB_INSTANCE, "StorageFindingReasonCode") => {
            Some("StorageFindingReasonCodes")
        }
        (RESOURCE_TYPE_RDS_DB_INSTANCE, "Idle") => Some("Idle"),
        _ => None,
    }
}

/// Whether `row` satisfies every filter of the request that asked for the export.
///
/// A filter selects on the member its name names ([`filter_column_of`]) and the row matches
/// when that column holds any of the filter's values; `RecommendationSourceType` selects on
/// the resource type the job exports, which is the same for every row. Several filters are
/// ANDed and a filter carrying no values constrains nothing. A filter naming a member the
/// surface's own recommendation does not declare selects nothing, which is what the
/// recommendation readers of this service answer for the same filter sets.
fn row_matches_filters(
    row: &ExportRow,
    resource_type: &str,
    filters: &[(String, Vec<String>)],
) -> bool {
    filters.iter().all(|(name, values)| {
        if values.is_empty() {
            return true;
        }
        let held = if name == "RecommendationSourceType" {
            resource_type
        } else {
            match filter_column_of(resource_type, name) {
                Some(column) => row.get(column).map_or("", String::as_str),
                None => return false,
            }
        };
        held.split(LIST_SEPARATOR)
            .any(|item| !item.is_empty() && values.iter().any(|value| value == item))
    })
}

/// An instant as the export files state it: RFC 3339 in UTC, the form every timestamp this
/// service reports on the wire takes.
///
/// Stated as the READERS state it. A recommendation reader carries this same instant in the
/// epoch-seconds number the modelled timestamp form is, so the file states the instant that
/// number denotes: formatting the unrounded value instead lets one stored instant read two
/// ways, because a sub-microsecond tick does not survive that number's precision and the
/// file would then sit a tick away from the answer the API gave for the same record.
fn timestamp(at: f64) -> String {
    let on_the_wire = ::aws_smithy_types::DateTime::from_secs_f64(at).as_secs_f64();
    ::aws_smithy_types::DateTime::from_secs_f64(on_the_wire)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// Record `value` for `field` in `row`, unless it is empty: a field the analysis records no
/// value for has no entry at all, so its cell is empty whatever column order a request asks
/// for.
fn put(row: &mut ExportRow, field: &str, value: impl Into<String>) {
    let value: String = value.into();
    if !value.is_empty() {
        row.insert(field.to_string(), value);
    }
}

/// [`put`] for a derived figure, at the two decimal places every figure this service reports
/// is rounded to.
fn put_number(row: &mut ExportRow, field: &str, value: f64) {
    put(row, field, round_hundredths(value).to_string());
}

/// [`put`] for a list-valued member, as one cell ([`LIST_SEPARATOR`]).
fn put_list<T: AsRef<str>>(row: &mut ExportRow, field: &str, values: &[T]) {
    let joined: Vec<&str> = values.iter().map(AsRef::as_ref).collect();
    put(row, field, joined.join(LIST_SEPARATOR));
}

/// The columns every resource type's export file shares, from the preferences the
/// recommendation was generated under: `EffectiveRecommendationPreferences*` and the
/// look-back period in days, which is the length of the period in force.
fn put_preferences(row: &mut ExportRow, preferences: &EffectivePreferences) {
    put_number(
        row,
        "LookbackPeriodInDays",
        look_back_period_in_days(&preferences.look_back_period),
    );
    put(
        row,
        "EffectiveRecommendationPreferencesLookBackPeriod",
        preferences.look_back_period.clone(),
    );
    put(
        row,
        "EffectiveRecommendationPreferencesInferredWorkloadTypes",
        preferences.inferred_workload_types.clone(),
    );
    put(
        row,
        "EffectiveRecommendationPreferencesEnhancedInfrastructureMetrics",
        preferences.enhanced_infrastructure_metrics.clone(),
    );
    // `savingsEstimationMode` is an `InstanceSavingsEstimationMode`, whose one member is the
    // `source` the money figures came from — which is what the reader reports for it.
    put(
        row,
        "EffectiveRecommendationPreferencesSavingsEstimationMode",
        SAVINGS_ESTIMATION_SOURCE,
    );
    let preferred: Vec<String> = preferences
        .preferred_resources
        .iter()
        .flat_map(|resource| resource.include_list.clone().unwrap_or_default())
        .collect();
    put_list(
        row,
        "EffectiveRecommendationPreferencesPreferredResources",
        &preferred,
    );
}

/// The exportable field one utilization metric lands under —
/// `<prefix><MetricName><Statistic>` — as the resource type's own `Exportable*Field` enum
/// (`fields`) declares it.
///
/// The field sets spell every metric name in UpperCamelCase, which is how the metric-name
/// enums spell most of their own values — but not all of them: the `IdleMetricName` value is
/// `CPU` while the field is `UtilizationMetricsCpuMaximum`, and `MetricName` names the series
/// an instance type does not size in SCREAMING_SNAKE_CASE (`NETWORK_IN_BYTES_PER_SECOND`) while
/// the field is `UtilizationMetricsNetworkInBytesPerSecondMaximum`. So a name is converted to
/// that casing before the field is looked up, and a name already in it is left as it is.
///
/// The STATISTIC a field's name carries is the one the export file reports that metric under,
/// which need not be the statistic a recommendation reports it with: this API "returns
/// utilization metrics using only the `Maximum` statistic" while a field set names, say, a
/// cache hit count `UtilizationMetricsCacheHitsSum`. So the field named for the OBSERVED
/// statistic is used wherever the enum declares it — which is what picks between the two
/// fields a metric carrying one field per statistic has — and otherwise the single field that
/// enum declares for the metric, so an observed metric lands in the column that names it
/// rather than in no column at all. A metric the enum declares no unambiguous field for keeps
/// the statistic-named field, which no column of that resource type reads, so nothing is
/// reported under a name the model does not give it.
fn utilization_field(fields: &[&str], prefix: &str, name: &str, statistic: &str) -> String {
    let name = if name == "CPU" {
        "Cpu".to_string()
    } else if name.contains('_') {
        name.split('_')
            .filter(|word| !word.is_empty())
            .map(|word| {
                let mut letters = word.chars();
                letters.next().map_or_else(String::new, |first| {
                    first.to_uppercase().collect::<String>()
                        + &letters.as_str().to_lowercase()
                })
            })
            .collect()
    } else {
        name.to_string()
    };
    let stem = format!("{prefix}{name}");
    let named = format!("{stem}{statistic}");
    if fields.contains(&named.as_str()) {
        return named;
    }
    let mut declared = fields.iter().filter(|field| {
        field
            .strip_prefix(stem.as_str())
            .is_some_and(|statistic| statistic.starts_with(char::is_uppercase))
    });
    match (declared.next(), declared.next()) {
        (Some(only), None) => (*only).to_string(),
        _ => named,
    }
}

/// The `Exportable*Field` an export file names a resource's own ARN under: each resource
/// type's enum declares the member its recommendation reports the ARN in
/// (`InstanceArn`, `VolumeArn`, `FunctionArn`, `ServiceArn`, `AutoScalingGroupArn`), and the
/// types whose recommendation reports it as `resourceArn` name it `ResourceArn`.
fn arn_field_of(resource_type: &str) -> &'static str {
    match resource_type {
        RESOURCE_TYPE_AUTO_SCALING_GROUP => "AutoScalingGroupArn",
        RESOURCE_TYPE_EC2_INSTANCE => "InstanceArn",
        RESOURCE_TYPE_EBS_VOLUME => "VolumeArn",
        RESOURCE_TYPE_LAMBDA_FUNCTION => "FunctionArn",
        RESOURCE_TYPE_ECS_SERVICE => "ServiceArn",
        _ => "ResourceArn",
    }
}

impl ComAmazonawsComputeoptimizerComputeoptimizerserviceState {
    /// Create the export job `plan` describes, with the two objects it writes when the clock
    /// makes it complete, and answer the record as stored.
    ///
    /// The files are computed HERE, from the analysis as it stands at `now`, so the export is
    /// of the recommendations the account held when it asked for the export rather than of
    /// whatever the analysis says when the job happens to complete.
    pub fn record_export_job(&mut self, plan: &ExportPlan, now: f64) -> ExportJob {
        let rows = self.export_rows(plan);
        let job = ExportJob {
            job_id: plan.job_id.clone(),
            region: plan.region.clone(),
            resource_type: plan.resource_type.clone(),
            bucket: plan.bucket.clone(),
            key: plan.key.clone(),
            metadata_key: plan.metadata_key.clone(),
            created_at: now,
            pending: Some(PendingExport {
                export_file: export_csv(&plan.columns, &rows),
                metadata_file: export_metadata(plan, rows.len(), now),
            }),
            failure_reason: None,
        };
        self.export_jobs.insert(job.clone());
        job
    }

    /// The recommendation rows one export file holds: those of the job's resource type that
    /// the account's analysis holds and the request's filters select, in analysis order.
    ///
    /// The rows are the ONE account this emulator presents
    /// ([`engine::emulator_account_id`]): `accountIds` and `includeMemberAccounts` select
    /// whose recommendations go into the file, an admitted request can name no account but
    /// this one ([`create_export_job`]), and this emulator records no organization and holds
    /// no other account's analysis, so neither of them selects any row away or adds one —
    /// they are stated in the metadata file as what the export was asked for.
    ///
    /// Compute Optimizer reports recommendations for an ENROLLED account only, and only once
    /// the analysis its enrollment starts is due, so an export created before then holds the
    /// header row alone — exactly what the recommendation readers report at that moment.
    fn export_rows(&self, plan: &ExportPlan) -> Vec<ExportRow> {
        if !self.enrollment.analysis_visible() {
            return Vec::new();
        }
        // When the analysis these rows are of was generated: the instant every reader of this
        // service reports for it ([`Enrollment::last_analysis_refresh`]), so a file states the
        // same instant the reader reports.
        let last_refresh = self
            .enrollment
            .last_analysis_refresh()
            .map(|generated| generated.as_secs_f64());
        let mut rows = match plan.resource_type.as_str() {
            RESOURCE_TYPE_AUTO_SCALING_GROUP => self.auto_scaling_group_export_rows(
                &plan.region,
                &plan.cpu_vendor_architectures,
                last_refresh,
            ),
            RESOURCE_TYPE_IDLE => self.idle_export_rows(&plan.region, last_refresh),
            RESOURCE_TYPE_LICENSE => self.license_export_rows(&plan.region, last_refresh),
            RESOURCE_TYPE_EC2_INSTANCE => self.instance_export_rows(
                &plan.region,
                &plan.cpu_vendor_architectures,
                last_refresh,
            ),
            RESOURCE_TYPE_RDS_DB_INSTANCE => self.rds_database_export_rows(
                &plan.analysed_databases,
                &plan.region,
                &plan.cpu_vendor_architectures,
                last_refresh,
            ),
            resource_type => {
                self.analysed_resource_export_rows(resource_type, &plan.region, last_refresh)
            }
        };
        rows.retain(|row| row_matches_filters(row, &plan.resource_type, &plan.filters));
        rows
    }

    /// The export rows of the Auto Scaling group analysis: one per recommendation option of
    /// every analysed group that the request's CPU vendor architecture preference admits, since
    /// each `RecommendationOptions*` field names ONE option's value — and one row carrying the
    /// group's own columns for a group the preference leaves no candidate of.
    ///
    /// A row names each group by the ARN the endpoint the export request addressed reports it
    /// under ([`AnalyzedAutoScalingGroup::arn_in`]), the same ARN the recommendation reader
    /// answers that region with.
    fn auto_scaling_group_export_rows(
        &self,
        region: &str,
        requested_architectures: &[String],
        last_refresh: Option<f64>,
    ) -> Vec<ExportRow> {
        let mut rows: Vec<ExportRow> = Vec::new();
        for group in self.visible_auto_scaling_group_analyses() {
            let arn = group.arn_in(region);
            let Some(parsed) = parse_resource_arn(&arn) else {
                continue;
            };
            let preferences = self.effective_preferences(&arn, parsed);
            // The candidates the preference admits, and "the CPU vendor and architecture for an
            // instance or Auto Scaling group recommendations" the exported options then consist
            // of: the crate's one rule for both ([`architecture_admitted`]), which is what the
            // reader of these recommendations applies — "when you specify AWS_ARM64 with an
            // ExportAutoScalingGroupRecommendations request, Compute Optimizer exports
            // recommendations that consist of Graviton instance types only".
            let options: Vec<&GroupConfiguration> = group
                .candidates
                .iter()
                .filter(|candidate| {
                    architecture_admitted(requested_architectures, &candidate.architecture)
                })
                .collect();
            let architectures = stated_architectures(
                options.iter().map(|option| option.architecture.as_str()),
            );
            if options.is_empty() {
                rows.push(auto_scaling_group_row(
                    group,
                    &arn,
                    &preferences,
                    None,
                    &architectures,
                    last_refresh,
                ));
                continue;
            }
            for option in options {
                rows.push(auto_scaling_group_row(
                    group,
                    &arn,
                    &preferences,
                    Some(option),
                    &architectures,
                    last_refresh,
                ));
            }
        }
        rows
    }

    /// The export rows of the Amazon EC2 instance analysis: one per recommendation option of
    /// every analysed instance that the request's CPU vendor architecture preference admits,
    /// since each `RecommendationOptions*` field names ONE option's value — and one row
    /// carrying the instance's own columns for an instance the preference leaves no candidate
    /// of.
    ///
    /// Every cell is computed from the same stored observation the recommendation READER
    /// reports the instance from ([`ObservedInstance`]) rather than from the findings-catalog
    /// row, which records none of an instance's own configuration or of its options, so an
    /// exported row and a `GetEC2InstanceRecommendations` response cannot disagree about which
    /// options the recommendation consists of.
    fn instance_export_rows(
        &self,
        region: &str,
        requested_architectures: &[String],
        last_refresh: Option<f64>,
    ) -> Vec<ExportRow> {
        let account_id = ::engine::emulator_account_id();
        let mut rows: Vec<ExportRow> = Vec::new();
        for instance in self
            .visible_instances()
            .filter(|instance| instance.account_id == account_id)
        {
            let arn = instance.instance_arn(region);
            let preferences =
                self.effective_preferences_for(&arn, RESOURCE_TYPE_EC2_INSTANCE, account_id);
            // The candidates the preference admits, and "the CPU vendor and architecture for an
            // instance or Auto Scaling group recommendations" the exported options then consist
            // of: the crate's one rule for both ([`architecture_admitted`]), which is what the
            // reader of these recommendations applies — "when you specify AWS_ARM64 with an
            // ExportEC2InstanceRecommendations request, Compute Optimizer exports
            // recommendations that consist of Graviton instance types only".
            let options: Vec<&InstanceConfiguration> = instance
                .candidates
                .iter()
                .filter(|candidate| {
                    architecture_admitted(requested_architectures, &candidate.architecture)
                })
                .collect();
            let architectures =
                stated_architectures(options.iter().map(|option| option.architecture.as_str()));
            // "Describes if an Amazon EC2 instance is idle": it is exactly when this service
            // also holds an IDLE recommendation for it, which is the record the reader of these
            // recommendations reports it from.
            let idle = self.idle_recommendations.get(&arn).is_some();
            if options.is_empty() {
                rows.push(instance_row(
                    instance,
                    &arn,
                    &preferences,
                    None,
                    &architectures,
                    idle,
                    last_refresh,
                ));
                continue;
            }
            for option in options {
                rows.push(instance_row(
                    instance,
                    &arn,
                    &preferences,
                    Some(option),
                    &architectures,
                    idle,
                    last_refresh,
                ));
            }
        }
        rows
    }

    /// The export rows of the findings catalog, for a resource type whose recommendation is
    /// reported from it: one row per analysed resource, carrying the columns the catalog row
    /// records ([`analysed_resource_row`]).
    fn analysed_resource_export_rows(
        &self,
        resource_type: &str,
        region: &str,
        last_refresh: Option<f64>,
    ) -> Vec<ExportRow> {
        let account_id = ::engine::emulator_account_id();
        let by_type = self.analysed_resources_by_type(account_id);
        by_type
            .get(resource_type)
            .map(|resources| {
                resources
                    .iter()
                    .map(|resource| {
                        let arn = resource.arn_in(region);
                        let preferences =
                            self.effective_preferences_for(&arn, resource_type, account_id);
                        analysed_resource_row(
                            resource,
                            resource_type,
                            &arn,
                            &preferences,
                            last_refresh,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The export rows of the Amazon Aurora and RDS database analysis: one per analysed
    /// database, carrying the members `GetRDSDatabaseRecommendations` reports for it
    /// ([`rds_database_row`]).
    ///
    /// A database's recommendation carries the engine, the DB instance class, the storage
    /// configuration, the utilization observed on it and the instance and storage options the
    /// analysis would move it to — none of which is a column of the findings-catalog row every
    /// other findings reader shares, so this surface builds its rows from the recommendation
    /// itself rather than from that row.
    ///
    /// `databases` are the analysed databases the plan resolved
    /// ([`ExportPlan::analysed_databases`]), which is the SAME selection
    /// `GetRDSDatabaseRecommendations` reads, so a file and a read of that moment hold the
    /// same databases.
    fn rds_database_export_rows(
        &self,
        databases: &[AnalysedResource],
        region: &str,
        requested_architectures: &[String],
        last_refresh: Option<f64>,
    ) -> Vec<ExportRow> {
        databases
            .iter()
            .map(|resource| {
                // The recommendation is built under the CPU vendor architectures the EXPORT
                // request asked for, exactly as the reader builds it under the ones a read
                // asks for, so the file's `EffectiveRecommendationPreferences*` columns state
                // the preferences the exported recommendation was generated under.
                let recommendation =
                    self.rds_recommendation(resource, region, Some(requested_architectures));
                rds_database_row(&recommendation, last_refresh)
            })
            .collect()
    }

    /// The export rows of the commercial software license inventory: one per licensed
    /// instance the analysis observes, from the SAME observation
    /// `GetLicenseRecommendations` reports ([`ObservedLicensedInstance`]) rather than from the
    /// findings catalog, which records no license configuration.
    fn license_export_rows(&self, region: &str, last_refresh: Option<f64>) -> Vec<ExportRow> {
        let account_id = ::engine::emulator_account_id();
        self.visible_licenses()
            .filter(|license| license.account_id == account_id)
            .map(|license| {
                let arn = license.instance_arn(region);
                let preferences =
                    self.effective_preferences_for(&arn, RESOURCE_TYPE_LICENSE, account_id);
                license_row(license, &arn, region, &preferences, last_refresh)
            })
            .collect()
    }

    /// The export rows of the idle resource analysis: one per resource the analysis found
    /// idle, with the utilization it observed as the evidence for the finding.
    fn idle_export_rows(&self, region: &str, last_refresh: Option<f64>) -> Vec<ExportRow> {
        let account_id = ::engine::emulator_account_id();
        self.visible_idle_recommendations()
            .filter(|analysis| analysis.account_id == account_id)
            .map(|analysis| {
                let arn = analysis.arn_in(region);
                let preferences = self.effective_preferences_for(&arn, RESOURCE_TYPE_IDLE, account_id);
                idle_row(analysis, &arn, &preferences, last_refresh)
            })
            .collect()
    }
}

/// One Auto Scaling group recommendation option as an export row.
///
/// Every cell is the value the recommendation READER reports for that member, computed from
/// the stored analysis the same way ([`AnalyzedAutoScalingGroup`]), so an exported row and a
/// `GetAutoScalingGroupRecommendations` response cannot disagree. The fields this analysis
/// records nothing for are left empty: the reserved-instance prices and the storage and
/// network of a configuration (the analysis prices an instance-hour on public on-demand
/// pricing alone), the GPU accelerator settings and the GPU utilization (no instance type in
/// this analysis has an attached accelerator), the EBS, disk and network utilization (the
/// analysis observes CPU and memory), and an option's estimated instance-hour reduction
/// (which "is only available for EC2 Auto Scaling groups with scaling policies", and this
/// analysis records no scaling policy).
fn auto_scaling_group_row(
    group: &AnalyzedAutoScalingGroup,
    arn: &str,
    preferences: &EffectivePreferences,
    option: Option<&GroupConfiguration>,
    architectures: &[String],
    last_refresh: Option<f64>,
) -> ExportRow {
    let mut row = ExportRow::new();
    put(&mut row, "AccountId", group.account_id.clone());
    put(&mut row, "AutoScalingGroupArn", arn.to_string());
    put(&mut row, "AutoScalingGroupName", group.name.clone());
    put(&mut row, "Finding", group.finding());
    put(
        &mut row,
        "CurrentPerformanceRisk",
        group.current_performance_risk.clone(),
    );
    for metric in group.utilization_metrics() {
        let (Some(name), Some(statistic)) = (metric.name, metric.statistic) else {
            continue;
        };
        put_number(
            &mut row,
            &utilization_field(
                EXPORTABLE_AUTO_SCALING_GROUP_FIELD_VALUES,
                "UtilizationMetrics",
                &name,
                &statistic,
            ),
            metric.value,
        );
    }
    put_configuration(&mut row, "CurrentConfiguration", &group.current);
    put_number(&mut row, "CurrentVCpus", group.current.vcpus);
    put_number(&mut row, "CurrentMemory", group.current.memory_gib);
    put_number(&mut row, "CurrentOnDemandPrice", group.current.hourly_price);
    put_preferences(&mut row, preferences);
    put_list(
        &mut row,
        "EffectiveRecommendationPreferencesCpuVendorArchitectures",
        architectures,
    );
    if preferences.infers_workload_types() {
        put_list(
            &mut row,
            "InferredWorkloadTypes",
            &group.inferred_workload_types,
        );
    }
    if let Some(at) = last_refresh {
        put(&mut row, "LastRefreshTimestamp", timestamp(at));
    }

    let Some(option) = option else {
        return row;
    };
    put_configuration(&mut row, "RecommendationOptionsConfiguration", option);
    put_number(&mut row, "RecommendationOptionsVcpus", option.vcpus);
    put_number(&mut row, "RecommendationOptionsMemory", option.memory_gib);
    put_number(
        &mut row,
        "RecommendationOptionsOnDemandPrice",
        option.hourly_price,
    );
    put_number(
        &mut row,
        "RecommendationOptionsPerformanceRisk",
        option.performance_risk,
    );
    for metric in group.projected_utilization_metrics(option) {
        let (Some(name), Some(statistic)) = (metric.name, metric.statistic) else {
            continue;
        };
        put_number(
            &mut row,
            &utilization_field(
                EXPORTABLE_AUTO_SCALING_GROUP_FIELD_VALUES,
                "RecommendationOptionsProjectedUtilizationMetrics",
                &name,
                &statistic,
            ),
            metric.value,
        );
    }
    put(
        &mut row,
        "RecommendationOptionsMigrationEffort",
        group.migration_effort(option, preferences.infers_workload_types()),
    );
    let (saving, percentage) = group.savings(option, 0.0);
    put_number(
        &mut row,
        "RecommendationOptionsSavingsOpportunityPercentage",
        percentage,
    );
    put_number(
        &mut row,
        "RecommendationOptionsEstimatedMonthlySavingsValue",
        saving,
    );
    put(
        &mut row,
        "RecommendationOptionsEstimatedMonthlySavingsCurrency",
        CURRENCY_USD,
    );
    // The after-discount figures are reported only where the account activated the
    // preference, which is what "activates" savings net of Savings Plans and Reserved
    // Instances discounts.
    if preferences.after_discounts() {
        let (saving, percentage) = group.savings(option, group.savings_plans_discount_rate);
        put_number(
            &mut row,
            "RecommendationOptionsSavingsOpportunityAfterDiscountsPercentage",
            percentage,
        );
        put_number(
            &mut row,
            "RecommendationOptionsEstimatedMonthlySavingsValueAfterDiscounts",
            saving,
        );
        put(
            &mut row,
            "RecommendationOptionsEstimatedMonthlySavingsCurrencyAfterDiscounts",
            CURRENCY_USD,
        );
    }
    row
}

/// One Auto Scaling group configuration's columns, under the prefix the field names give it
/// (`CurrentConfiguration…` for the group's own, `RecommendationOptionsConfiguration…` for an
/// option's): the members `AutoScalingGroupConfiguration` reports
/// ([`GroupConfiguration::to_wire`]).
fn put_configuration(row: &mut ExportRow, prefix: &str, configuration: &GroupConfiguration) {
    let wire = configuration.to_wire();
    put(
        row,
        &format!("{prefix}InstanceType"),
        wire.instance_type.unwrap_or_default(),
    );
    put_list(
        row,
        &format!("{prefix}MixedInstanceTypes"),
        &wire.mixed_instance_types.unwrap_or_default(),
    );
    put(row, &format!("{prefix}Type"), wire.r#type.unwrap_or_default());
    put(
        row,
        &format!("{prefix}AllocationStrategy"),
        wire.allocation_strategy.unwrap_or_default(),
    );
    put(
        row,
        &format!("{prefix}DesiredCapacity"),
        wire.desired_capacity.to_string(),
    );
    put(row, &format!("{prefix}MinSize"), wire.min_size.to_string());
    put(row, &format!("{prefix}MaxSize"), wire.max_size.to_string());
}

/// One findings-catalog row as an export row: the columns a catalog row records about any
/// analysed resource — whose account owns it, its ARN at the Region's endpoint, its finding
/// and the reason codes behind it, the risk its current configuration runs, what adopting the
/// recommendation saves, and the preferences the recommendation was generated under.
///
/// The columns specific to a resource type's own configuration are left empty here: they are
/// members of that type's recommendation rather than of the catalog row every findings reader
/// shares. The database surface, whose catalog row carries the configuration the analysis
/// recorded for it, reports those members from the recommendation itself
/// ([`rds_database_row`]).
///
/// A column whose value an effective recommendation preference switches off is left empty for
/// as long as that preference is off, the way the recommendation readers of the same analysis
/// leave the member unreported. The CPU vendor architectures of a resource's recommendation
/// options are not among the columns a catalog row fills: no resource type reported from the
/// catalog has an option carrying one — the instance surface, whose options do, builds its rows
/// from the observation itself ([`ComAmazonawsComputeoptimizerComputeoptimizerserviceState::instance_export_rows`]).
fn analysed_resource_row(
    resource: &AnalysedResource,
    resource_type: &str,
    arn: &str,
    preferences: &EffectivePreferences,
    last_refresh: Option<f64>,
) -> ExportRow {
    let mut row = ExportRow::new();
    put(&mut row, "AccountId", resource.account_id.clone());
    put(&mut row, arn_field_of(resource_type), arn);
    put(&mut row, "Finding", resource.finding.clone());
    put_list(
        &mut row,
        "FindingReasonCodes",
        &resource.finding_reason_codes,
    );
    put(
        &mut row,
        "CurrentPerformanceRisk",
        resource.current_performance_risk.clone(),
    );
    put_number(
        &mut row,
        "RecommendationOptionsSavingsOpportunityPercentage",
        percentage_of(resource.monthly_savings(), resource.current_monthly_cost),
    );
    put_number(
        &mut row,
        "RecommendationOptionsEstimatedMonthlySavingsValue",
        resource.monthly_savings(),
    );
    put(
        &mut row,
        "RecommendationOptionsEstimatedMonthlySavingsCurrency",
        CURRENCY_USD,
    );
    // "The applications that might be running on the resource as inferred by Compute
    // Optimizer" are reported only while the inference is active for the resource — the same
    // gate the recommendation readers apply, so the cell cannot hold types the account
    // switched off while the row's effective-preference cell says they are switched off.
    if preferences.infers_workload_types() {
        put_list(
            &mut row,
            "InferredWorkloadTypes",
            &resource.inferred_workload_types,
        );
    }
    put_preferences(&mut row, preferences);
    if let Some(at) = last_refresh {
        put(&mut row, "LastRefreshTimestamp", timestamp(at));
    }
    row
}

/// One Amazon EC2 instance recommendation option as an export row, or the instance's own
/// columns alone (`option` is `None`) where the request's preference admits no candidate of it.
///
/// Every cell is the value the recommendation READER reports for that member, computed from the
/// stored observation the same way ([`ObservedInstance`]), so an exported row and a
/// `GetEC2InstanceRecommendations` response cannot disagree. The fields this analysis records
/// nothing for are left empty: the reserved-instance prices and the storage and network of a
/// configuration (the analysis prices an instance-hour on public on-demand pricing alone), the
/// GPU accelerator settings and the GPU utilization (no instance type in this analysis has an
/// attached accelerator), and the utilization preferences, which are the preference RECORDS the
/// account wrote rather than a value this recommendation reports for the instance.
fn instance_row(
    instance: &ObservedInstance,
    arn: &str,
    preferences: &EffectivePreferences,
    option: Option<&InstanceConfiguration>,
    architectures: &[String],
    idle: bool,
    last_refresh: Option<f64>,
) -> ExportRow {
    let mut row = ExportRow::new();
    put(&mut row, "AccountId", instance.account_id.clone());
    put(&mut row, "InstanceArn", arn.to_string());
    put(
        &mut row,
        "InstanceName",
        instance.instance_name().unwrap_or_default(),
    );
    put(&mut row, "InstanceState", instance.instance_state.clone());
    put(&mut row, "Finding", instance.finding());
    put_list(
        &mut row,
        "FindingReasonCodes",
        &instance.finding_reason_codes,
    );
    put(
        &mut row,
        "CurrentPerformanceRisk",
        instance.current_performance_risk.clone(),
    );
    put(
        &mut row,
        "CurrentInstanceType",
        instance.current.instance_type.clone(),
    );
    put_number(&mut row, "CurrentVCpus", instance.current.vcpus);
    put_number(&mut row, "CurrentMemory", instance.current.memory_gib);
    put_number(
        &mut row,
        "CurrentOnDemandPrice",
        instance.current.hourly_price,
    );
    put(&mut row, "Idle", if idle { IDLE_TRUE } else { IDLE_FALSE });
    for metric in instance.utilization_metrics() {
        let (Some(name), Some(statistic)) = (metric.name, metric.statistic) else {
            continue;
        };
        put_number(
            &mut row,
            &utilization_field(
                EXPORTABLE_INSTANCE_FIELD_VALUES,
                "UtilizationMetrics",
                &name,
                &statistic,
            ),
            metric.value,
        );
    }
    // "An array of objects that describe the source resource of the recommendation": an
    // instance recommendation is computed from the analysis of that very instance, which is
    // the source the reader reports for it.
    put(
        &mut row,
        "RecommendationsSourcesRecommendationSourceArn",
        arn.to_string(),
    );
    put(
        &mut row,
        "RecommendationsSourcesRecommendationSourceType",
        RESOURCE_TYPE_EC2_INSTANCE,
    );
    put_preferences(&mut row, preferences);
    // "Describes the CPU vendor and architecture for an instance or Auto Scaling group
    // recommendations": the architectures the instance's exported options consist of.
    put_list(
        &mut row,
        "EffectiveRecommendationPreferencesCpuVendorArchitectures",
        architectures,
    );
    // The external metrics provider the account prefers, and Compute Optimizer's integration
    // status with it — the crate's one rule for that status ([`external_metric_status`]), so the
    // file and the reader cannot report different integrations.
    let source = preferences
        .external_metrics_preference
        .as_ref()
        .and_then(|preference| preference.source.as_deref());
    put(
        &mut row,
        "EffectiveRecommendationPreferencesExternalMetricsSource",
        source.unwrap_or_default(),
    );
    let status = external_metric_status(source);
    put(
        &mut row,
        "ExternalMetricStatusCode",
        status.status_code.unwrap_or_default(),
    );
    put(
        &mut row,
        "ExternalMetricStatusReason",
        status.status_reason.unwrap_or_default(),
    );
    // "The applications that might be running on the instance as inferred by Compute Optimizer"
    // are reported only while the inference is active for it — the same gate the reader applies.
    if preferences.infers_workload_types() {
        put_list(
            &mut row,
            "InferredWorkloadTypes",
            &instance.inferred_workload_types,
        );
    }
    let tags: Vec<String> = instance
        .tags
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    put_list(&mut row, "Tags", &tags);
    if let Some(at) = last_refresh {
        put(&mut row, "LastRefreshTimestamp", timestamp(at));
    }

    let Some(option) = option else {
        return row;
    };
    put(
        &mut row,
        "RecommendationOptionsInstanceType",
        option.instance_type.clone(),
    );
    put_number(&mut row, "RecommendationOptionsVcpus", option.vcpus);
    put_number(&mut row, "RecommendationOptionsMemory", option.memory_gib);
    put_number(
        &mut row,
        "RecommendationOptionsOnDemandPrice",
        option.hourly_price,
    );
    put_number(
        &mut row,
        "RecommendationOptionsPerformanceRisk",
        instance.option_performance_risk(option),
    );
    put_list(
        &mut row,
        "RecommendationOptionsPlatformDifferences",
        &instance.platform_differences(option),
    );
    put(
        &mut row,
        "RecommendationOptionsMigrationEffort",
        instance.migration_effort(option, preferences.infers_workload_types()),
    );
    for metric in instance.projected_utilization_metrics(option) {
        let (Some(name), Some(statistic)) = (metric.name, metric.statistic) else {
            continue;
        };
        put_number(
            &mut row,
            &utilization_field(
                EXPORTABLE_INSTANCE_FIELD_VALUES,
                "RecommendationOptionsProjectedUtilizationMetrics",
                &name,
                &statistic,
            ),
            metric.value,
        );
    }
    let (saving, percentage) = instance.savings(option);
    put_number(
        &mut row,
        "RecommendationOptionsSavingsOpportunityPercentage",
        percentage,
    );
    put_number(
        &mut row,
        "RecommendationOptionsEstimatedMonthlySavingsValue",
        saving,
    );
    put(
        &mut row,
        "RecommendationOptionsEstimatedMonthlySavingsCurrency",
        CURRENCY_USD,
    );
    // The after-discount figures are reported only where the account activated the preference,
    // which is what "activates" savings net of Savings Plans and Reserved Instances discounts.
    if preferences.after_discounts() {
        put_number(
            &mut row,
            "RecommendationOptionsSavingsOpportunityAfterDiscountsPercentage",
            percentage,
        );
        put_number(
            &mut row,
            "RecommendationOptionsEstimatedMonthlySavingsValueAfterDiscounts",
            saving,
        );
        put(
            &mut row,
            "RecommendationOptionsEstimatedMonthlySavingsCurrencyAfterDiscounts",
            CURRENCY_USD,
        );
    }
    row
}

/// The `ExportableRDSDBField` prefix the columns of ONE recommendation option carry: the DB
/// instance option's and the DB storage option's, each named for the member of
/// `RDSDBRecommendation` that option is reported in.
const INSTANCE_OPTION: &str = "InstanceRecommendationOptions";
/// See [`INSTANCE_OPTION`].
const STORAGE_OPTION: &str = "StorageRecommendationOptions";

/// One Amazon Aurora or RDS database recommendation as an export row: the value of every
/// `ExportableRDSDBField` the recommendation carries, taken from the member that field names.
///
/// `recommendation` is the one `GetRDSDatabaseRecommendations` reports for the same catalog
/// row, so an exported row and that reader's response cannot disagree about any column.
///
/// The fields that name no member of an `RDSDBRecommendation` are left empty, because the
/// analysis records no value for them: whether the database is a Multi-AZ deployment or the
/// writer of its cluster, and the per-hour and per-month PRICES of the current and recommended
/// configurations together with the cluster-level cost roll-ups — this analysis records a
/// database's cost per month as a whole, and the money the recommendation reports is what
/// adopting it saves.
fn rds_database_row(recommendation: &RDSDBRecommendation, last_refresh: Option<f64>) -> ExportRow {
    let text = |member: &Option<String>| member.clone().unwrap_or_default();
    let mut row = ExportRow::new();
    put(&mut row, "AccountId", text(&recommendation.account_id));
    put(&mut row, "ResourceArn", text(&recommendation.resource_arn));
    put(&mut row, "Engine", text(&recommendation.engine));
    put(
        &mut row,
        "EngineVersion",
        text(&recommendation.engine_version),
    );
    put(
        &mut row,
        "DBClusterIdentifier",
        text(&recommendation.db_cluster_identifier),
    );
    if let Some(tier) = recommendation.promotion_tier {
        put(&mut row, "PromotionTier", tier.to_string());
    }
    put(&mut row, "Idle", text(&recommendation.idle));
    put(
        &mut row,
        "CurrentDBInstanceClass",
        text(&recommendation.current_db_instance_class),
    );
    if let Some(configuration) = &recommendation.current_storage_configuration {
        put_storage_configuration(&mut row, "CurrentStorageConfiguration", configuration);
    }
    put(
        &mut row,
        "CurrentStorageEstimatedMonthlyVolumeIOPsCostVariation",
        text(
            &recommendation.current_storage_estimated_monthly_volume_io_ps_cost_variation,
        ),
    );
    put(
        &mut row,
        "CurrentInstancePerformanceRisk",
        text(&recommendation.current_instance_performance_risk),
    );
    put(
        &mut row,
        "InstanceFinding",
        text(&recommendation.instance_finding),
    );
    put(
        &mut row,
        "StorageFinding",
        text(&recommendation.storage_finding),
    );
    put_list(
        &mut row,
        "InstanceFindingReasonCodes",
        recommendation
            .instance_finding_reason_codes
            .as_deref()
            .unwrap_or_default(),
    );
    put_list(
        &mut row,
        "StorageFindingReasonCodes",
        recommendation
            .storage_finding_reason_codes
            .as_deref()
            .unwrap_or_default(),
    );
    put_utilization_metrics(
        &mut row,
        "UtilizationMetrics",
        recommendation.utilization_metrics.as_deref().unwrap_or_default(),
    );

    // The DB instance option: the class the analysis would move the database to, the
    // utilization projected onto it, the risk it then runs and what adopting it saves. Each
    // `*RecommendationOptions*` field names ONE option, and this analysis reports one option
    // per side, so both sides' columns go in the same row.
    if let Some(option) = recommendation
        .instance_recommendation_options
        .iter()
        .flatten()
        .next()
    {
        put(
            &mut row,
            "InstanceRecommendationOptionsDBInstanceClass",
            text(&option.db_instance_class),
        );
        put(
            &mut row,
            "InstanceRecommendationOptionsRank",
            option.rank.to_string(),
        );
        put_number(
            &mut row,
            "InstanceRecommendationOptionsPerformanceRisk",
            option.performance_risk,
        );
        put_utilization_metrics(
            &mut row,
            "InstanceRecommendationOptionsProjectedUtilizationMetrics",
            option
                .projected_utilization_metrics
                .as_deref()
                .unwrap_or_default(),
        );
        put_savings(&mut row, INSTANCE_OPTION, option.savings_opportunity.as_ref());
        if let Some(after) = &option.savings_opportunity_after_discounts {
            put_savings_after_discounts(
                &mut row,
                INSTANCE_OPTION,
                after.savings_opportunity_percentage,
                after
                    .estimated_monthly_savings
                    .as_ref()
                    .map(|savings| (savings.currency.clone().unwrap_or_default(), savings.value)),
            );
        }
    }

    // The DB storage option: the storage configuration the analysis would move the database
    // to, and what adopting it saves.
    if let Some(option) = recommendation
        .storage_recommendation_options
        .iter()
        .flatten()
        .next()
    {
        if let Some(configuration) = &option.storage_configuration {
            put_storage_configuration(&mut row, STORAGE_OPTION, configuration);
        }
        put(
            &mut row,
            "StorageRecommendationOptionsRank",
            option.rank.to_string(),
        );
        put(
            &mut row,
            "StorageRecommendationOptionsEstimatedMonthlyVolumeIOPsCostVariation",
            text(&option.estimated_monthly_volume_io_ps_cost_variation),
        );
        put_savings(&mut row, STORAGE_OPTION, option.savings_opportunity.as_ref());
        if let Some(after) = &option.savings_opportunity_after_discounts {
            put_savings_after_discounts(
                &mut row,
                STORAGE_OPTION,
                after.savings_opportunity_percentage,
                after
                    .estimated_monthly_savings
                    .as_ref()
                    .map(|savings| (savings.currency.clone().unwrap_or_default(), savings.value)),
            );
        }
    }

    // The preferences the recommendation was generated under, as the recommendation itself
    // reports them. The database field set declares a column for each member an
    // `RDSEffectiveRecommendationPreferences` carries.
    if let Some(preferences) = &recommendation.effective_recommendation_preferences {
        put_list(
            &mut row,
            "EffectiveRecommendationPreferencesCpuVendorArchitectures",
            preferences
                .cpu_vendor_architectures
                .as_deref()
                .unwrap_or_default(),
        );
        put(
            &mut row,
            "EffectiveRecommendationPreferencesEnhancedInfrastructureMetrics",
            text(&preferences.enhanced_infrastructure_metrics),
        );
        put(
            &mut row,
            "EffectiveRecommendationPreferencesLookBackPeriod",
            text(&preferences.look_back_period),
        );
        put(
            &mut row,
            "EffectiveRecommendationPreferencesSavingsEstimationMode",
            preferences
                .savings_estimation_mode
                .as_ref()
                .map(|mode| text(&mode.source))
                .unwrap_or_default(),
        );
    }
    put_number(
        &mut row,
        "LookbackPeriodInDays",
        recommendation.lookback_period_in_days,
    );
    let tags: Vec<String> = recommendation
        .tags
        .iter()
        .flatten()
        .map(|tag| {
            format!(
                "{}={}",
                tag.key.clone().unwrap_or_default(),
                tag.value.clone().unwrap_or_default()
            )
        })
        .collect();
    put_list(&mut row, "Tags", &tags);
    // "The timestamp of when the DB instance recommendation was last generated": the instant
    // the recommendation reports where the analysis recorded one, and otherwise the instant
    // this account's analysis was last refreshed, which is what every other export file of
    // this service states.
    let refreshed = recommendation
        .last_refresh_timestamp
        .map(|at| at.as_secs_f64())
        .or(last_refresh);
    if let Some(at) = refreshed {
        put(&mut row, "LastRefreshTimestamp", timestamp(at));
    }
    row
}

/// The columns one `DBStorageConfiguration` fills, under the `prefix` the field set names that
/// configuration with (`CurrentStorageConfiguration` for the database's own,
/// `StorageRecommendationOptions` for the one its storage option would move it to).
fn put_storage_configuration(
    row: &mut ExportRow,
    prefix: &str,
    configuration: &DBStorageConfiguration,
) {
    put(
        row,
        &format!("{prefix}StorageType"),
        configuration.storage_type.clone().unwrap_or_default(),
    );
    put(
        row,
        &format!("{prefix}AllocatedStorage"),
        configuration.allocated_storage.to_string(),
    );
    if let Some(iops) = configuration.iops {
        put(row, &format!("{prefix}IOPS"), iops.to_string());
    }
    if let Some(maximum) = configuration.max_allocated_storage {
        put(
            row,
            &format!("{prefix}MaxAllocatedStorage"),
            maximum.to_string(),
        );
    }
    if let Some(throughput) = configuration.storage_throughput {
        put(
            row,
            &format!("{prefix}StorageThroughput"),
            throughput.to_string(),
        );
    }
}

/// The savings columns of one recommendation option: the saving as a percentage of what the
/// resource costs today, and the amount and currency of that saving.
fn put_savings(row: &mut ExportRow, prefix: &str, opportunity: Option<&SavingsOpportunity>) {
    let Some(opportunity) = opportunity else {
        return;
    };
    put_number(
        row,
        &format!("{prefix}SavingsOpportunityPercentage"),
        opportunity.savings_opportunity_percentage,
    );
    if let Some(savings) = &opportunity.estimated_monthly_savings {
        put(
            row,
            &format!("{prefix}EstimatedMonthlySavingsCurrency"),
            savings.currency.clone().unwrap_or_default(),
        );
        put_number(
            row,
            &format!("{prefix}EstimatedMonthlySavingsValue"),
            savings.value,
        );
    }
}

/// The AFTER-DISCOUNTS savings columns of one recommendation option, which each option shape
/// carries under its own type but reports the same three figures in: the percentage, and the
/// currency and amount of the estimated monthly saving.
fn put_savings_after_discounts(
    row: &mut ExportRow,
    prefix: &str,
    percentage: f64,
    estimated: Option<(String, f64)>,
) {
    put_number(
        row,
        &format!("{prefix}SavingsOpportunityAfterDiscountsPercentage"),
        percentage,
    );
    if let Some((currency, value)) = estimated {
        put(
            row,
            &format!("{prefix}EstimatedMonthlySavingsCurrencyAfterDiscounts"),
            currency,
        );
        put_number(
            row,
            &format!("{prefix}EstimatedMonthlySavingsValueAfterDiscounts"),
            value,
        );
    }
}

/// The utilization columns of a database's observed or projected metrics: each metric under
/// the field that names it and the statistic it was measured with ([`utilization_field`]).
fn put_utilization_metrics(row: &mut ExportRow, prefix: &str, metrics: &[RDSDBUtilizationMetric]) {
    for metric in metrics {
        let (Some(name), Some(statistic)) = (metric.name.as_deref(), metric.statistic.as_deref())
        else {
            continue;
        };
        put_number(
            row,
            &utilization_field(
                EXPORTABLE_R_D_S_D_B_FIELD_VALUES,
                prefix,
                name,
                statistic,
            ),
            metric.value,
        );
    }
}

/// One commercial software license recommendation as an export row: the members
/// `LicenseRecommendation` reports, each computed from the stored observation the same way
/// `GetLicenseRecommendations` computes it — the license configuration in force, the finding
/// and the reason code behind it, and the one option the analysis has (the edition it would
/// move the license to) with what adopting it saves. A license with no price carries no
/// savings figure rather than an invented one.
fn license_row(
    license: &ObservedLicensedInstance,
    arn: &str,
    region: &str,
    preferences: &EffectivePreferences,
    last_refresh: Option<f64>,
) -> ExportRow {
    let mut row = ExportRow::new();
    put(&mut row, "AccountId", license.account_id.clone());
    put(&mut row, "ResourceArn", arn);
    put(&mut row, "Finding", license.finding());
    put_list(
        &mut row,
        "FindingReasonCodes",
        &license.finding_reason_codes(),
    );
    put_number(
        &mut row,
        "LookbackPeriodInDays",
        license.analysed_days(&preferences.look_back_period),
    );
    put(
        &mut row,
        "CurrentLicenseConfigurationInstanceType",
        license.instance_type.clone(),
    );
    put(
        &mut row,
        "CurrentLicenseConfigurationOperatingSystem",
        license.operating_system.clone(),
    );
    put(
        &mut row,
        "CurrentLicenseConfigurationLicenseName",
        license.license_name.clone(),
    );
    put(
        &mut row,
        "CurrentLicenseConfigurationLicenseEdition",
        license.license_edition.clone(),
    );
    put(
        &mut row,
        "CurrentLicenseConfigurationLicenseModel",
        license.license_model.clone(),
    );
    put(
        &mut row,
        "CurrentLicenseConfigurationLicenseVersion",
        license.license_version.clone(),
    );
    put(
        &mut row,
        "CurrentLicenseConfigurationNumberOfCores",
        license.number_of_cores.to_string(),
    );
    // The metric source is the CloudWatch Application Insights application that watches the
    // instance, which the reader reports as its provider and that application's ARN.
    let sources: Vec<String> = license
        .metrics_source(region)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|source| source.provider_arn)
        .collect();
    put_list(
        &mut row,
        "CurrentLicenseConfigurationMetricsSource",
        &sources,
    );
    if let Some(recommended) = license.recommended_license_edition() {
        put(&mut row, "RecommendationOptionsLicenseEdition", recommended);
        put(
            &mut row,
            "RecommendationOptionsLicenseModel",
            license.license_model.clone(),
        );
        put(
            &mut row,
            "RecommendationOptionsOperatingSystem",
            license.operating_system.clone(),
        );
    }
    if let Some(savings) = license.savings() {
        put_number(
            &mut row,
            "RecommendationOptionsSavingsOpportunityPercentage",
            savings.percentage,
        );
        put_number(
            &mut row,
            "RecommendationOptionsEstimatedMonthlySavingsValue",
            savings.monthly_amount,
        );
        put(
            &mut row,
            "RecommendationOptionsEstimatedMonthlySavingsCurrency",
            CURRENCY_USD,
        );
    }
    let tags: Vec<String> = license
        .tags
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    put_list(&mut row, "Tags", &tags);
    if let Some(at) = last_refresh {
        put(&mut row, "LastRefreshTimestamp", timestamp(at));
    }
    row
}

/// One idle resource recommendation as an export row: the members
/// `IdleRecommendation` reports ([`IdleResourceAnalysis::to_recommendation`]), each computed
/// from the stored analysis the same way the reader computes it.
fn idle_row(
    analysis: &IdleResourceAnalysis,
    arn: &str,
    preferences: &EffectivePreferences,
    last_refresh: Option<f64>,
) -> ExportRow {
    let look_back_days = analysis.analysed_days(&preferences.look_back_period);
    let (percentage, _value) = analysis.savings();
    let mut row = ExportRow::new();
    put(&mut row, "AccountId", analysis.account_id.clone());
    put(&mut row, "ResourceArn", arn);
    put(&mut row, "ResourceId", analysis.resource_id());
    put(
        &mut row,
        "ResourceType",
        analysis.resource_type().unwrap_or_default(),
    );
    put(&mut row, "Finding", analysis.finding.clone());
    put(
        &mut row,
        "FindingDescription",
        analysis.finding_description(look_back_days),
    );
    put_number(&mut row, "LookbackPeriodInDays", look_back_days);
    // `SavingsOpportunity` / `SavingsOpportunityAfterDiscount` are one column each, and what
    // an `IdleSavingsOpportunity` is named for is its percentage: "the estimated monthly
    // savings possible as a percentage of monthly cost".
    put_number(&mut row, "SavingsOpportunity", percentage);
    put_number(&mut row, "SavingsOpportunityAfterDiscount", percentage);
    for metric in &analysis.utilization_metrics {
        let (Some(name), Some(statistic)) = (metric.name.as_deref(), metric.statistic.as_deref())
        else {
            continue;
        };
        put_number(
            &mut row,
            &utilization_field(
                EXPORTABLE_IDLE_FIELD_VALUES,
                "UtilizationMetrics",
                name,
                statistic,
            ),
            metric.value,
        );
    }
    let tags: Vec<String> = analysis
        .tags
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    put_list(&mut row, "Tags", &tags);
    if let Some(at) = last_refresh {
        put(&mut row, "LastRefreshTimestamp", timestamp(at));
    }
    row
}

/// Write the export objects of every job the clock has already made due, and start this
/// store's clock task for the job the calling export has just recorded: what every
/// `Export*Recommendations` operation does once it has recorded its job, so none of them
/// differs on it.
pub fn deliver_and_schedule<Eng: Send + Sync + 'static>(engine: &Arc<Eng>, now: f64) {
    write_due_exports(&**engine, now);
    drive_export_clock(engine, 2 * super::EXPORT_JOB_STEP);
}

/// Write the export objects of every job the clock has made `Complete` and that has not been
/// written yet.
///
/// What the stored due instant has ALREADY made due, and nothing else: a job still `Queued`
/// or `InProgress` at `now` keeps its objects, so a client reading the destination before the
/// job completes finds nothing there. Writing is idempotent — the objects are TAKEN from the
/// job, so two callers racing write them once.
///
/// A destination that will not take an object (the bucket has been removed since the export
/// was created, say) FAILS the job: "The reason for an export job failure" is what
/// `failureReason` reports, and the job's status is then `Failed` rather than `Complete`.
/// When the object store cannot be reached at all nothing is taken, so the writes stay owed.
pub fn write_due_exports<Eng: Send + Sync + 'static>(engine: &Eng, now: f64) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let Some(objects) = s3::state::service_state(engine) else {
        ::tracing::warn!("the object store is not reachable from this engine, so no \
                          recommendations export was written");
        return;
    };
    // The due objects are taken out of this service's store and the lock released before the
    // object store is locked: the two locks are never held at once.
    let due: Vec<DueExport> = state.lock().export_jobs.take_due_exports(now);
    if due.is_empty() {
        return;
    }

    let mut failures: Vec<(String, String)> = Vec::new();
    {
        let mut store = objects.lock();
        for export in due {
            if let Some(reason) = write_export_objects(&mut store, &export) {
                failures.push((export.job_id, reason));
            }
        }
    }
    if !failures.is_empty() {
        let mut guard = state.lock();
        for (job_id, reason) in failures {
            guard.export_jobs.fail(&job_id, reason);
        }
    }
}

/// Write one job's two objects, answering the reason the destination did not take them.
///
/// Both objects are attempted whatever the other did, so a destination that took one of them
/// keeps it: an export file without its metadata, or the other way round, is what a client
/// reading the bucket then finds beside the `Failed` job the reason is recorded against. The
/// reason names the first object the destination refused.
fn write_export_objects(
    store: &mut s3::state::ComAmazonawsS3Amazons3State,
    export: &DueExport,
) -> Option<String> {
    let bucket = export.bucket.as_str();
    let mut refused: Option<String> = None;
    for (key, body, content_type) in [
        (
            export.key.as_str(),
            export.pending.export_file.as_str(),
            "text/csv",
        ),
        (
            export.metadata_key.as_str(),
            export.pending.metadata_file.as_str(),
            "application/json",
        ),
    ] {
        let object = s3::state::Object::stored(
            body.as_bytes().to_vec(),
            Some(content_type.to_string()),
        );
        let outcome = store.put_object(bucket, key, object);
        if let Some(reason) = refusal_reason(bucket, key, outcome) {
            refused = refused.or(Some(reason));
        }
    }
    refused
}

/// Why the destination did not take the object written to `key`, or `None` when it did.
///
/// Worded from the CONDITION the object store reported rather than from its own types: this
/// is what a client reads as the job's `failureReason`.
fn refusal_reason(
    bucket: &str,
    key: &str,
    outcome: Option<s3::state::ObjectWrite>,
) -> Option<String> {
    use s3::state::ObjectWrite;
    let condition = match outcome {
        Some(ObjectWrite::Created | ObjectWrite::Replaced) => return None,
        // The bucket existed when the export job was created and does not any more.
        None => format!("the destination Amazon S3 bucket {bucket} does not exist"),
        Some(ObjectWrite::KeyTooLong { bytes }) => {
            format!("the object key is {bytes} bytes long, which Amazon S3 does not accept")
        }
        Some(ObjectWrite::ObjectCountReached | ObjectWrite::TotalObjectCountReached) => {
            format!("the destination Amazon S3 bucket {bucket} cannot hold another object")
        }
        Some(
            ObjectWrite::ByteBudgetReached { .. } | ObjectWrite::TotalByteBudgetReached { .. },
        ) => format!(
            "the destination Amazon S3 bucket {bucket} cannot hold the bytes of the export"
        ),
        Some(ObjectWrite::MetadataTooLarge { bytes }) => {
            format!("the object's metadata is {bytes} bytes, which Amazon S3 does not accept")
        }
    };
    Some(format!(
        "Compute Optimizer could not write the recommendations export to {key} because \
         {condition}."
    ))
}

/// Start the one short in-process task that writes this job's objects when the clock makes
/// the job complete.
///
/// A client that exports and then downloads the file makes no further Compute Optimizer call,
/// so a write performed only by the next call could be a write never performed. The task does
/// not change WHEN the objects appear: it only calls [`write_due_exports`], which applies
/// what the stored due instant has already made due, so a client reading the destination
/// before that instant still finds nothing. With no async runtime to spawn onto there is no
/// task, and the objects are written by the next operation instead.
pub fn drive_export_clock<Eng: Send + Sync + 'static>(engine: &Arc<Eng>, due_in: ::std::time::Duration) {
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let engine = Arc::clone(engine);
    runtime.spawn(async move {
        ::tokio::time::sleep(due_in).await;
        write_due_exports(&*engine, super::now_secs());
    });
}
