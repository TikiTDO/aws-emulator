// SPDX-License-Identifier: CC0-1.0

//! The data automation library INGESTION JOB this store holds: the record
//! `InvokeDataAutomationLibraryIngestionJob` mints, the status the clock owes it, and the
//! two projections `GetDataAutomationLibraryIngestionJob` and
//! `ListDataAutomationLibraryIngestionJobs` serve.
//!
//! RULE. An ingestion job is a resource OF the one library its `libraryArn` names
//! (`smithy_query(query="operation",operation="InvokeDataAutomationLibraryIngestionJob")`
//! binds that ARN as the `@httpLabel` of the job collection's path,
//! `/data-automation-libraries/{libraryArn}/library-ingestion-jobs/`): a submission mints
//! one under that library, a read of the (library, job) pair reports it, and a listing of
//! that library enumerates exactly the jobs minted under it — never a job of another
//! library — in this service's listing order ([`super::in_listing_order`]), one page at a
//! time. A job is accepted [`INGESTION_JOB_IN_PROGRESS`] and reaches
//! [`INGESTION_JOB_COMPLETED`] once the clock has made that transition due, from which
//! instant every read of it reports the same `completionTime`.
//!
//! Single-homed here because all three operations project the same record: three copies
//! of that projection are three readings waiting to drift.

use std::time::{Duration, SystemTime};

use super::ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState as ServiceState;
use crate::ops::types::{
    DataAutomationLibraryIngestionJob, DataAutomationLibraryIngestionJobSummary,
    InputConfiguration, NotificationConfiguration, OutputConfiguration, Tag,
};

/// The status an ingestion job carries from the moment
/// `InvokeDataAutomationLibraryIngestionJob` accepts it.
///
/// Source: `smithy_query(shape="LibraryIngestionJobStatus")` -> `["IN_PROGRESS",
/// "COMPLETED", "COMPLETED_WITH_ERRORS", "FAILED"]`. The submission is asynchronous — it
/// answers the job's ARN rather than its result — so the status that holds when it
/// returns is the in-flight one.
pub const INGESTION_JOB_IN_PROGRESS: &str = "IN_PROGRESS";

/// The terminal status of an ingestion job this store accepted. See
/// [`INGESTION_JOB_IN_PROGRESS`] for the source of the value.
///
/// The two error statuses of that set (`COMPLETED_WITH_ERRORS`, `FAILED`) are reached by
/// no path here: nothing in this emulator fails an ingestion job it has accepted, and a
/// submission this service refuses is refused on the wire rather than recorded as a
/// failed job.
pub const INGESTION_JOB_COMPLETED: &str = "COMPLETED";

/// How long after `InvokeDataAutomationLibraryIngestionJob` the clock owes a job its
/// `IN_PROGRESS` -> `COMPLETED` transition.
///
/// AWS documents no duration for it — the submission is modelled as an async invoke whose
/// result is read back through `GetDataAutomationLibraryIngestionJob`, precisely because
/// the job is not finished when the call returns — so this span is CHOSEN, not modelled.
/// It is deliberately non-zero, so a caller reading the job straight after the submission
/// still observes `IN_PROGRESS` and the status it reports is never skipped. Nothing sleeps
/// for it: the due instant is stored on the record and compared against the clock.
pub const INGESTION_JOB_SPAN: Duration = Duration::from_secs(3);

/// How many ingestion jobs the store holds before
/// `InvokeDataAutomationLibraryIngestionJob` refuses with the modelled
/// `ServiceQuotaExceededException` (HTTP 402).
///
/// Same reasoning as [`super::LIBRARY_QUOTA`]: Bedrock Data Automation publishes
/// adjustable quotas per account per Region and this emulator models neither Service
/// Quotas nor an account's adjustments, so this is the emulator's OWN bound on the store —
/// what stops a caller submitting jobs from growing the state without end.
pub const INGESTION_JOB_QUOTA: usize = 1_000;

/// The largest page `ListDataAutomationLibraryIngestionJobs` serves, and the page size it
/// uses when the request names no `maxResults`. The same value as
/// [`super::LIBRARY_PAGE_LIMIT`] because it has the same source: this listing binds
/// `maxResults` to the one modelled `MaxResults` shape
/// (`smithy_query(query="operation",operation="ListDataAutomationLibraryIngestionJobs")`
/// -> `@range(min: 1, max: 1000)`), so no caller can ask for more than this and an absent
/// `maxResults` cannot serialise the whole store either.
pub const INGESTION_JOB_PAGE_LIMIT: i32 = super::LIBRARY_PAGE_LIMIT;

/// [`super::LIBRARY_ARN_NOT_FOUND_MESSAGE`] for the resource this module holds: what a
/// read answers when a `jobArn` it was handed names no ingestion job of the library the
/// request addressed. The resource kind is named the way this service names it
/// (`Ingestion Job`, the word its tagging refusals use for this kind), and nothing is
/// taken from the request.
pub const INGESTION_JOB_ARN_NOT_FOUND_MESSAGE: &str = "Ingestion Job Arn not found.";

/// The resource portion prefix of an ingestion job ARN, as the modelled
/// `DataAutomationLibraryIngestionJobArn` pattern spells it
/// (`smithy_query(query="operation",operation="InvokeDataAutomationLibraryIngestionJob")`:
/// `…:data-automation-library-ingestion-job/[a-zA-Z0-9-]{12,36}`).
const INGESTION_JOB_RESOURCE_PREFIX: &str = "data-automation-library-ingestion-job/";

/// What `InvokeDataAutomationLibraryIngestionJob` answers for a submission whose
/// `inputConfiguration` names neither of the two sources it can read.
///
/// The modelled `InputConfiguration` carries `s3Object` and `inlinePayload` and makes
/// NEITHER required (`smithy_query(query="shape",shape="InputConfiguration")`), so the
/// pair cannot be refused by the constraint walk that runs before the handler: a
/// configuration that names no source describes nothing to ingest, and only the service
/// can say so. Fixed text naming the member and the two sources, with nothing taken from
/// the request.
pub const MISSING_INGESTION_INPUT_MESSAGE: &str =
    "inputConfiguration must specify either s3Object or inlinePayload.";

/// What `InvokeDataAutomationLibraryIngestionJob` answers once the store holds
/// [`INGESTION_JOB_QUOTA`] jobs: this service's quota voice (the sentence form its other
/// quota refusals take), with the quota itself read off the constant rather than spelled
/// twice.
#[must_use]
pub fn quota_message() -> String {
    format!(
        "the quota of {INGESTION_JOB_QUOTA} data automation library ingestion jobs per \
         account has been reached"
    )
}

/// Mint a fresh ingestion job ARN for `region`.
///
/// `DataAutomationLibraryIngestionJobArn` is
/// `^arn:aws(|-cn|-iso|-iso-[a-z]|-us-gov):bedrock:[a-zA-Z0-9-]*:[0-9]{12}:data-automation-library-ingestion-job/[a-zA-Z0-9-]{12,36}$`
/// with `@length(..128)`, and both the submission's response member and the read's
/// `@httpLabel` carry those constraints — so an ARN this service mints has to satisfy them,
/// or it would name a job no caller could address. The resource id is the one
/// [`super::mint_data_automation_library_arn`] builds and rests its uniqueness on (the
/// creating instant's nanoseconds-within-the-second, then the store's never-rolled-back
/// sequence number in full 64-bit width), and `region` is reduced to a region label through
/// the shared `service_common::arn::region_label` grammar — non-empty, at most 20
/// characters, only `[a-z0-9-]` — so the whole value stays inside the pattern and well
/// under 128 characters whatever a caller signs for.
#[must_use]
pub fn mint_ingestion_job_arn(
    region: &str,
    created_at: ::aws_smithy_types::DateTime,
    sequence: u64,
) -> String {
    let id = format!("{:08x}-{:016x}", created_at.subsec_nanos(), sequence);
    ::engine::emulator_arn(
        super::ARN_SERVICE,
        ::service_common::arn::region_label(region).unwrap_or(super::DEFAULT_REGION),
        &format!("{INGESTION_JOB_RESOURCE_PREFIX}{id}"),
    )
}

/// A stored data automation library ingestion job: the submission
/// `InvokeDataAutomationLibraryIngestionJob` accepted, the library it was submitted
/// against, and the instant the clock owes its completion.
#[derive(Debug, Clone)]
pub struct DataAutomationLibraryIngestionJobRecord {
    pub job_arn: String,
    /// The library this job was submitted against, which is the only library whose
    /// listing enumerates it and whose read resolves it.
    pub library_arn: String,
    pub entity_type: String,
    pub operation_type: String,
    /// The submission's input, as submitted: what the job reads.
    pub input_configuration: InputConfiguration,
    pub output_configuration: OutputConfiguration,
    pub notification_configuration: Option<NotificationConfiguration>,
    pub creation_time: ::aws_smithy_types::DateTime,
    /// When this job stops being [`INGESTION_JOB_IN_PROGRESS`]. Every reader derives
    /// [`Self::status_at`] from it rather than storing a status a read would have to
    /// write.
    pub completes_at: SystemTime,
    /// The same instant as the modelled `date-time` a completed job reports for
    /// `completionTime`, so two reads of a completed job cannot report two instants.
    pub completion_time: ::aws_smithy_types::DateTime,
    pub tags: Vec<Tag>,
}

impl DataAutomationLibraryIngestionJobRecord {
    /// This job's modeled `jobStatus` as of `now`: in flight until the transition the
    /// submission owed falls due, completed from then on.
    ///
    /// A pure function of the stored due instant, so reporting it writes nothing and two
    /// readers at the same instant agree.
    #[must_use]
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        if now >= self.completes_at {
            INGESTION_JOB_COMPLETED
        } else {
            INGESTION_JOB_IN_PROGRESS
        }
    }

    /// The `completionTime` this job reports as of `now`: the instant it completed on once
    /// it has, and nothing while it is still in flight — a job that has not finished has
    /// no completion instant to report, which is why the modelled member is optional where
    /// `creationTime` is required.
    #[must_use]
    pub fn completion_time_at(&self, now: SystemTime) -> Option<::aws_smithy_types::DateTime> {
        (now >= self.completes_at).then_some(self.completion_time)
    }

    /// Project the record onto the modeled `DataAutomationLibraryIngestionJob` shape a
    /// read serves, with the status the clock has made due at `now`.
    ///
    /// INFALLIBLE: every member of the projection is a member of the record, so no request
    /// can make this fail and no handler needs an unwrap for it.
    #[must_use]
    pub fn to_job(&self, now: SystemTime) -> DataAutomationLibraryIngestionJob {
        DataAutomationLibraryIngestionJob {
            job_arn: self.job_arn.clone(),
            creation_time: self.creation_time,
            entity_type: self.entity_type.clone(),
            operation_type: self.operation_type.clone(),
            job_status: self.status_at(now).to_string(),
            output_configuration: self.output_configuration.clone(),
            completion_time: self.completion_time_at(now),
            // Nothing here fails a job it accepted, so a job carries no error to report;
            // the two error members stay absent rather than carrying an invented reason.
            error_message: None,
            error_type: None,
        }
    }

    /// Project the record onto the NARROW `DataAutomationLibraryIngestionJobSummary` item
    /// a listing serves.
    ///
    /// `smithy_query(query="shape",shape="DataAutomationLibraryIngestionJobSummary")`
    /// carries the job's identity, its status, the two members that say what it does and
    /// the two instants — every one of them filled from the same record, and the status and
    /// completion instant from the same `now` a read of the job would use, so a listing and
    /// a read at one instant cannot disagree about a job.
    #[must_use]
    pub fn to_summary(&self, now: SystemTime) -> DataAutomationLibraryIngestionJobSummary {
        DataAutomationLibraryIngestionJobSummary {
            job_arn: self.job_arn.clone(),
            job_status: self.status_at(now).to_string(),
            entity_type: self.entity_type.clone(),
            operation_type: self.operation_type.clone(),
            creation_time: self.creation_time,
            completion_time: self.completion_time_at(now),
        }
    }
}

impl super::ListedResource for DataAutomationLibraryIngestionJobRecord {
    fn order_key(&self) -> (i64, u32, &str) {
        (
            self.creation_time.secs(),
            self.creation_time.subsec_nanos(),
            self.job_arn.as_str(),
        )
    }
}

impl ServiceState {
    /// The ingestion jobs of ONE library, in this service's listing order (`creationTime`,
    /// then ARN) — the order [`super::in_listing_order`] gives every listing of this
    /// service.
    ///
    /// Scoped to the library the job was submitted against, so a listing of one library
    /// never serves a job of another.
    pub fn library_ingestion_jobs_in_order(
        &self,
        library_arn: &str,
    ) -> Vec<&DataAutomationLibraryIngestionJobRecord> {
        super::in_listing_order(
            self.ingestion_jobs
                .values()
                .filter(|record| record.library_arn == library_arn),
        )
    }

    /// The ingestion job `job_arn` names, if this store holds it UNDER the library
    /// `library_arn` addressed.
    ///
    /// A job is a resource of one library, so a pair naming a job of a different library
    /// resolves to nothing: the read is of the pair, never of the job alone.
    pub fn library_ingestion_job(
        &self,
        library_arn: &str,
        job_arn: &str,
    ) -> Option<&DataAutomationLibraryIngestionJobRecord> {
        self.ingestion_jobs
            .get(job_arn)
            .filter(|record| record.library_arn == library_arn)
    }
}
