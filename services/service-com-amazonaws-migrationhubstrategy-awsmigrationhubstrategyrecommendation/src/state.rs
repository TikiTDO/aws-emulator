// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-migrationhubstrategy-awsmigrationhubstrategyrecommendation service.
//!
//! Strategy Recommendations has two primary resources behind its import-then-assess
//! loop, and both live here:
//!
//! * an **import file task**, minted by `StartImportFileTask` and reported by
//!   `GetImportFileTask` one at a time and by `ListImportFileTask` a page at a time —
//!   including the status report object that import wrote into the report bucket the
//!   request named, which is what its `statusReportS3Key` names
//!   ([`import_status_report_key`]);
//! * an **assessment**, minted by `StartAssessment`, reported by `GetAssessment` and
//!   `GetLatestAssessmentId`, and moved to a terminal status by `StopAssessment`.
//!
//! Every one of those operations computes its answer from the record below — there is
//! exactly one copy of each resource, and the `DataCollectionDetails` an assessment
//! reports is derived from the counters it stores, never canned.
//!
//! Beside them sits the account's **server inventory** ([`ServerRecord`]), which
//! `ListServers` reports: the servers an import file task ingested, each carrying the
//! import groups it came in with, the recommendation the service holds for it and the
//! strategy option the account prefers for it — what `UpdateServerConfig` writes and
//! `GetServerStrategies` reports. A server a collector fetched also carries the discovery
//! data that fetch brought with it ([`ServerDiscovery`]), which is what
//! `ListAnalyzableServers` reports.
//!
//! The recommendation a server or an application component holds is what an ASSESSMENT
//! produced: an assessment that reaches `COMPLETE` has analysed the portfolio it covered,
//! so at that instant every resource it covered is given the recommendation the account's
//! saved preferences select for it and the instant it was analysed
//! ([`RegionState::analyze_covered_resources`], and [`analysis`] for what the preferences
//! select). WHICH resources an assessment covers is what its criteria select
//! ([`RegionState::servers_covered_by`]).
//!
//! Beside the servers sit the **data collectors** installed in the account's environment
//! ([`CollectorRecord`]), which `ListCollectors` reports a page at a time: each one is
//! reported exactly as its registration recorded it, so that read derives nothing from the
//! request that makes it.
//!
//! Beside the servers, the account's **application components**
//! ([`ApplicationComponentRecord`]) — the processes and databases the assessment found
//! running on those servers — which `GetApplicationComponentDetails` reports one at a
//! time and `ListApplicationComponents` a filtered page at a time: every member of the
//! `ApplicationComponentDetail` it answers comes from that
//! record, and the association counts it reports are counted off the record's own lists.
//! `UpdateApplicationComponentConfig` writes that same record — the component's app type,
//! inclusion status, database credentials, source code configuration and the strategy
//! option the account prefers for it — and `GetApplicationComponentStrategies` reports
//! that preference beside the service's own recommendation, so each read answers what the
//! update stored.
//!
//! Strategy Recommendations is a REGIONAL service, so none of those resources is held
//! once for the account: the store holds one [`RegionState`] per Region, and a resource
//! created through one Region's endpoint is reported only by reads signed for that same
//! Region. Every handler resolves its request's Region with [`request_region`] and then
//! reaches that Region's portfolio alone.
//!
//! And beside those, the account's one **portfolio preferences** record
//! ([`PortfolioPreferences`]): what `PutPortfolioPreferences` saves and
//! `GetPortfolioPreferences` reports, member for member — and what the analysis of the
//! portfolio is computed from, so they are read as well as written.
//!
//! Last, the account's **recommendation report generation tasks**
//! ([`RecommendationReport`]): what `StartRecommendationReportGeneration` mints — one at
//! a time, a start call made while one is still in flight coalescing onto that task and
//! answering its id ([`RegionState::ongoing_report_generation`]) rather than minting a
//! second — and `GetRecommendationReportDetails` reports. A task's artifact is the report
//! FILE the start call writes into the bucket the account named for what this service
//! uploads ([`RegionState::report_data_bucket`]), in the `OutputFormat` the call asked for
//! ([`report_file`]); the completed task names that object, and a task that produced none
//! names no location at all.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AnalyzableServerSummary, AppUnitError, ApplicationComponentDetail,
    ApplicationComponentStatusSummary, ApplicationComponentStrategy, ApplicationComponentSummary,
    ApplicationPreferences,
    AssessmentSummary,
    AssessmentTarget, AssociatedApplication, Collector, ConfigurationSummary, ConflictException,
    DataCollectionDetails, DatabaseConfigDetail,
    DatabasePreferences, GetImportFileTaskOutput, GetPortfolioPreferencesOutput, Group, ImportFileTaskInformation,
    OSInfo, PrioritizeBusinessGoals, RecommendationReportDetails, RecommendationSet,
    ResourceNotFoundException, ServerDetail, ServerError, ServerStatusSummary, ServerStrategy,
    ServerSummary, SourceCodeRepository,
    StrategyOption, StrategySummary, SystemInfo, TransformationTool, ValidationException,
    GROUP_NAME_VALUES, SERVER_OS_TYPE_VALUES, STRATEGY_VALUES, TARGET_DESTINATION_VALUES,
    TRANSFORMATION_TOOL_NAME_VALUES,
};

/// The Region partitioning of this store: the per-service state struct itself, the Region
/// a request is served in, and the ceiling on how many Regions the store keeps a
/// portfolio for.
pub mod regions;

/// What the analysis of a portfolio produces, computed from the account's saved
/// preferences and the resources' own stored members.
pub mod analysis;

/// The import file an import file task reads, and the records it ingests from it into the
/// portfolio the read path reports.
pub mod import_file;

/// A member's modelled `@pattern` as a requirement on the whole value, checked over an
/// operation's input before the handler reads it.
pub mod patterns;

/// The recommendation report FILE a report generation task writes into the account's
/// report data bucket, and the key it is written under.
pub mod report_file;

pub use regions::{
    request_region, ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState,
    DEFAULT_REGION, MAX_REGIONS,
};

/// `ImportFileTaskStatus`'s non-terminal import status: the status a task carries from
/// the instant `StartImportFileTask` returns until the clock owes it a terminal one.
pub const IMPORT_IN_PROGRESS: &str = "ImportInProgress";

/// The terminal `ImportFileTaskStatus` an import file task that ingested every record its
/// file declared reaches.
///
/// Reaching a terminal status is what makes the records the import ingested part of the
/// portfolio: the resources the import file declared were read when the start call ran
/// ([`import_file`]) and enter the inventory the read path reports when the clock owes the
/// task that status ([`RegionState::settle_imports`]), which is also when
/// [`ImportFileTask::number_of_records_success`] /
/// [`ImportFileTask::number_of_records_failed`] report what it ingested. WHICH of the three
/// terminal statuses a task reaches is what its ingestion produced
/// ([`import_file::Ingestion::terminal_status`]).
pub const IMPORT_SUCCESS: &str = "ImportSuccess";

/// The terminal `ImportFileTaskStatus` an import that ingested some of the records its
/// file declared and failed on others reaches.
pub const IMPORT_PARTIAL_SUCCESS: &str = "ImportPartialSuccess";

/// The terminal `ImportFileTaskStatus` an import that ingested nothing it was asked to
/// reaches: its file could not be read at all, or every record the file declared failed.
///
/// It is NOT the status of an import whose file declared no resource: that import did
/// everything its file asked of it, so it reaches [`IMPORT_SUCCESS`] with both record
/// counts at zero.
pub const IMPORT_FAILED: &str = "ImportFailed";

/// `AssessmentStatus`'s non-terminal value: an assessment is collecting data.
pub const ASSESSMENT_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `AssessmentStatus` an assessment that ran to the end reaches.
pub const ASSESSMENT_COMPLETE: &str = "COMPLETE";

/// The `AssessmentStatus` `StopAssessment` moves a running assessment to.
pub const ASSESSMENT_STOPPED: &str = "STOPPED";

/// The `RunTimeAssessmentStatus` a server of the portfolio carries while the assessment
/// covering it is collecting data from it.
///
/// `ServerDetail$dataCollectionStatus` is documented "The status of assessment for the
/// server", so it is the per-server face of the assessment's own `AssessmentStatus`: an
/// assessment reported `IN_PROGRESS` is collecting, and the collection task on each
/// server it covers has started.
pub const COLLECTION_STARTED: &str = "dataCollectionTaskStarted";

/// The `RunTimeAssessmentStatus` a server still collecting carries once a caller stopped
/// the assessment — the per-server face of [`ASSESSMENT_STOPPED`].
pub const COLLECTION_STOPPED: &str = "dataCollectionTaskStopped";

/// The `RunTimeAssessmentStatus` a server still collecting carries once the assessment
/// ran to the end — the per-server face of [`ASSESSMENT_COMPLETE`].
pub const COLLECTION_SUCCESS: &str = "dataCollectionTaskSuccess";

/// The `RunTimeAssessmentStatus` a server carries from the moment an import brings it
/// into the portfolio: no assessment has covered it yet, so the collection task on it is
/// still to be scheduled.
pub const COLLECTION_TO_BE_SCHEDULED: &str = "dataCollectionTaskToBeScheduled";

/// The `RunTimeAssessmentStatus` values that mean a collection task is RUNNING on the
/// server, and so the ones a terminal assessment status moves off that server.
///
/// `dataCollectionTaskToBeScheduled` is not one of them: nothing is collecting for a
/// server whose task was never scheduled, so a stop or a completion leaves it where it
/// was, as it does a server that already reached a terminal status of its own.
pub const COLLECTION_RUNNING: &[&str] = &["dataCollectionTaskScheduled", COLLECTION_STARTED];

/// `RecommendationReportStatus`'s non-terminal value: the status a report generation
/// task carries from the instant `StartRecommendationReportGeneration` returns until the
/// clock owes it a terminal one.
pub const REPORT_IN_PROGRESS: &str = "IN_PROGRESS";

/// The terminal `RecommendationReportStatus` a report generation task that ran to the
/// end reaches.
///
/// It is the status the service's recorded behaviour shows a started task settling on (a
/// task is `IN_PROGRESS` and then `SUCCESS`), so a task of this emulator settles there
/// too. The artifact that status means is DELIVERED: the report is an Excel or JSON FILE
/// the service uploads to the account's report data bucket, so the start call generates it
/// and writes it there ([`report_file`]) and the completed task names it —
/// [`RecommendationReport::s3_bucket`] and [`RecommendationReport::s3_keys`]. An account
/// that named no bucket for the reports this service generates
/// ([`RegionState::report_data_bucket`]) has nowhere for the file to go, so that task
/// names no location at all rather than one nothing wrote — the same rule the import's
/// status report follows ([`ImportFileTask::status_report_s3_key`]).
pub const REPORT_SUCCESS: &str = "SUCCESS";

/// The `OutputFormat` a report generation task takes when the request names none:
/// `StartRecommendationReportGenerationRequest$outputFormat` is documented "The output
/// format for the recommendation report file. The default format is Microsoft Excel",
/// and `Excel` is that format's value in `OutputFormat`.
pub const DEFAULT_OUTPUT_FORMAT: &str = "Excel";

/// How long an import file task stays `ImportInProgress`, an assessment `IN_PROGRESS`,
/// and a recommendation report generation task `IN_PROGRESS`, before the clock owes it a
/// terminal status.
///
/// Neither AWS nor the model publishes a duration for any of them, so the emulator
/// chooses one span for all three. It is deliberately NOT zero, so a caller acting
/// straight after the start call still observes the non-terminal status (and
/// `StopAssessment` still has a running assessment to stop). The due instant is stored on
/// the record and materialised by whichever later call reads it — nothing sleeps.
pub const COMPLETION_SPAN: Duration = Duration::from_millis(500);

/// How many import file tasks the store holds before `StartImportFileTask` answers its
/// modelled `ServiceQuotaExceededException` ("The AWS account has reached its quota of
/// imports.").
///
/// AWS publishes an adjustable quota this emulator does not model, so this is the
/// emulator's OWN bound on the store — far above any workflow it serves, and what stops
/// the state growing without end.
pub const IMPORT_TASK_QUOTA: usize = 1_000;

/// The same bound for assessments, refused with `StartAssessment`'s own modelled
/// `ServiceQuotaExceededException`.
pub const ASSESSMENT_QUOTA: usize = 1_000;

/// How many recommendation report generation tasks the store holds before
/// `StartRecommendationReportGeneration` refuses to mint another.
///
/// The same emulator-owned bound as [`IMPORT_TASK_QUOTA`] and [`ASSESSMENT_QUOTA`], for
/// the same reason: it is what stops the store growing without end. The operation models
/// no quota error, so the refusal is its modelled `ThrottlingException` (HTTP 429) — the
/// request is denied because too many have been made.
pub const REPORT_QUOTA: usize = 1_000;

/// The reason a report generation task has no report file to name, when the account named
/// no bucket for the reports this service generates: there is nowhere for the file to go,
/// so the task names no location at all.
///
/// It is the service's own words for the condition, carrying nothing from the request and
/// nothing about what serves it, and it is interpolated into the task's `statusMessage`
/// once the clock gives the task its terminal status ([`RecommendationReport::settle`]).
pub const NO_REPORT_DATA_BUCKET: &str =
    "this account has configured no Amazon S3 bucket for report data";

/// The reason a report generation task has no report file to name, when the account's
/// report data bucket would not take the object — it no longer holds a bucket of that
/// name, say.
pub const REPORT_FILE_NOT_WRITTEN: &str =
    "the report file could not be written to the account's report data bucket";

/// The reason a report generation task has no report file to name, when the portfolio its
/// filter selected is larger than one report file covers
/// ([`report_file::MAX_REPORT_ROWS`]).
pub const REPORT_SCOPE_TOO_LARGE: &str =
    "the portfolio in scope is larger than one recommendation report file covers";

/// How many `groupIdFilter` entries `StartRecommendationReportGeneration` accepts.
///
/// `GroupIds` declares no `@length`, so the boundary validator admits a list of any size;
/// this is the emulator's OWN bound on the work one request can ask for, and a filter
/// past it is refused with the service's invalid-request-body sentence rather than
/// silently truncated — the same treatment a `maxResults` past its documented maximum
/// gets ([`MAX_LIST_PAGE_SIZE`]).
pub const MAX_GROUP_FILTER_ENTRIES: usize = 100;

/// How many criterion VALUES `StartAssessment` accepts across the criteria of one
/// assessment.
///
/// `AssessmentTargets` bounds the criteria at ten, but `AssessmentTargetValues` declares no
/// `@length`, so the boundary validator admits a criterion listing values without end —
/// and every one of them is compared to every server of the portfolio when the scope is
/// resolved ([`RegionState::servers_covered_by`]). This is the emulator's OWN bound on that
/// work, the same bound and the same refusal [`MAX_GROUP_FILTER_ENTRIES`] gives the other
/// selection a start call carries: past it the request is refused with the service's
/// invalid-request-body sentence, decided before the inventory is read, rather than
/// silently truncated.
pub const MAX_ASSESSMENT_TARGET_VALUES: usize = 100;

/// One import file task, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct ImportFileTask {
    /// Minted by [`ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState::mint_import_file_task_id`],
    /// unique across every Region's portfolio.
    pub id: String,
    /// `StartImportFileTaskRequest$name`, reported as `importName`.
    pub import_name: String,
    /// `StartImportFileTaskRequest$S3Bucket`, reported as `inputS3Bucket`.
    pub input_s3_bucket: String,
    /// `StartImportFileTaskRequest$s3key`, reported as `inputS3Key`.
    pub input_s3_key: String,
    /// `StartImportFileTaskRequest$dataSourceType`: where the servers come from.
    pub data_source_type: Option<String>,
    /// `StartImportFileTaskRequest$groupId`: the groups this import's resources join.
    pub group_id: Vec<Group>,
    /// `StartImportFileTaskRequest$s3bucketForReportData` — the bucket the import
    /// results are uploaded to.
    pub status_report_s3_bucket: Option<String>,
    /// The key of the status report object this import wrote inside
    /// [`Self::status_report_s3_bucket`], set by the start call once the object is
    /// there. `None` when the request named no report bucket, or when the object could
    /// not be written: a key is only ever held for an object a caller can read.
    pub status_report_s3_key: Option<String>,
    /// One of `ImportFileTaskStatus`'s values.
    pub status: String,
    /// The terminal `ImportFileTaskStatus` the clock owes this task — what its ingestion
    /// produced ([`import_file::Ingestion::terminal_status`]), settled by the start call
    /// that ran the import and written onto [`Self::status`] when the completion comes
    /// due.
    pub terminal_status: String,
    pub start_time: SystemTime,
    pub completion_time: Option<SystemTime>,
    pub number_of_records_success: i32,
    pub number_of_records_failed: i32,
    /// The instant the clock owes this task its terminal status, while it has one.
    pub completion_due: Option<SystemTime>,
    /// The servers this import ingested from its file, held here until the task reaches
    /// its terminal status and they enter the inventory
    /// ([`RegionState::settle_imports`]).
    ///
    /// The read path reports the portfolio of a FINISHED import, so a task still
    /// importing holds its records rather than publishing them: the records themselves
    /// (ids included) were settled by the start call, so completing the task only moves
    /// them.
    pub pending_servers: Vec<ServerRecord>,
    /// The application components it ingested, held for the same span and for the same
    /// reason.
    pub pending_components: Vec<ApplicationComponentRecord>,
}

impl ImportFileTask {
    /// Materialise the completion the clock owes this task, if `now` is past its due
    /// instant.
    ///
    /// The status it reaches is [`Self::terminal_status`], the one its ingestion produced,
    /// so a task that read nothing does not report the status of one that imported
    /// everything. `completion_time` is the DUE instant rather than `now`, so a task
    /// reports the same completion time however late it is first read.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(due) = self.completion_due else {
            return;
        };
        if now < due {
            return;
        }
        self.status = self.terminal_status.clone();
        self.completion_time = Some(due);
        self.completion_due = None;
    }

    /// True once this task reached a terminal `ImportFileTaskStatus`.
    pub fn is_complete(&self) -> bool {
        self.completion_due.is_none()
    }

    /// The `GetImportFileTask` answer for this task.
    ///
    /// The record's own values, with the three completion members reported only once
    /// the task completed: a running import has no completion time and no record counts
    /// yet.
    pub fn to_output(&self) -> GetImportFileTaskOutput {
        let complete = self.is_complete();
        GetImportFileTaskOutput {
            id: Some(self.id.clone()),
            status: Some(self.status.clone()),
            start_time: Some(::aws_smithy_types::DateTime::from(self.start_time)),
            input_s3_bucket: Some(self.input_s3_bucket.clone()),
            input_s3_key: Some(self.input_s3_key.clone()),
            status_report_s3_bucket: self.status_report_s3_bucket.clone(),
            // The status report is the artifact of a FINISHED import, so its key is
            // reported with the other completion members: a task still importing has
            // no final report for a caller to read per-record outcomes from.
            status_report_s3_key: complete
                .then(|| self.status_report_s3_key.clone())
                .flatten(),
            completion_time: self
                .completion_time
                .map(::aws_smithy_types::DateTime::from),
            number_of_records_success: complete.then_some(self.number_of_records_success),
            number_of_records_failed: complete.then_some(self.number_of_records_failed),
            import_name: Some(self.import_name.clone()),
        }
    }

    /// The `ListImportFileTask` entry for this task.
    ///
    /// The same record the single-task read reports, in the list shape: both answers
    /// come from these fields, so a task cannot read one way through one operation and
    /// another way through the other.
    pub fn to_task_information(&self) -> ImportFileTaskInformation {
        let reported = self.to_output();
        ImportFileTaskInformation {
            id: reported.id,
            status: reported.status,
            start_time: reported.start_time,
            input_s3_bucket: reported.input_s3_bucket,
            input_s3_key: reported.input_s3_key,
            status_report_s3_bucket: reported.status_report_s3_bucket,
            status_report_s3_key: reported.status_report_s3_key,
            completion_time: reported.completion_time,
            number_of_records_success: reported.number_of_records_success,
            number_of_records_failed: reported.number_of_records_failed,
            import_name: reported.import_name,
        }
    }
}

/// One page of import file tasks, as `ListImportFileTask` answers it.
#[derive(Debug, Default)]
pub struct ImportTaskPage {
    /// The entries of this page, in the store's total order.
    pub task_infos: Vec<ImportFileTaskInformation>,
    /// The cursor the next call resumes from, set only when another task follows this
    /// page.
    pub next_token: Option<String>,
}

/// The largest page a list operation of this service serves, and the page size it uses
/// when the caller names none.
///
/// Every `maxResults` member the service takes documents the same bound —
/// `ListImportFileTaskRequest$maxResults` "The total number of items to return. The
/// maximum value is 100.", `ListServersRequest$maxResults` "The maximum number of items
/// to include in the response. The maximum value is 100." The members carry no `@range`,
/// so the boundary validator passes any integer and the bound is enforced where the page
/// is taken.
pub const MAX_LIST_PAGE_SIZE: i32 = 100;

/// How many records the page a `maxResults` asks for holds, as [`MAX_LIST_PAGE_SIZE`]'s rule
/// decides it — the ONE place this service decides it, for every list it serves.
///
/// An absent `maxResults` takes the documented maximum; a value inside
/// `1..=`[`MAX_LIST_PAGE_SIZE`] is taken as asked; anything else is refused with
/// [`invalid_request_body`] rather than silently clamped, so a caller that asked for more
/// than the service gives has to learn it cannot have it. Each operation lifts that
/// `ValidationException` into its own error channel.
pub fn page_size(max_results: Option<i32>) -> Result<usize, ValidationException> {
    let requested = max_results.unwrap_or(MAX_LIST_PAGE_SIZE);
    if !(1..=MAX_LIST_PAGE_SIZE).contains(&requested) {
        return Err(invalid_request_body());
    }
    // An accepted value is inside 1..=MAX_LIST_PAGE_SIZE, so the conversion is infallible
    // here; refusing the impossible case keeps it from inventing a page size of its own.
    usize::try_from(requested).map_err(|_| invalid_request_body())
}

/// The marker a `ListImportFileTask` cursor carries, so a token minted elsewhere (or
/// invented by a caller) is recognised as foreign rather than resolved against this
/// store.
pub const IMPORT_TASK_PAGE_MARKER: &str = "migrationhubstrategy:import-file-task:";

/// The marker a `ListServers` cursor carries.
///
/// A token names a position in ONE list, so each list has its own marker and a token
/// issued for another list is foreign here — the two never resolve against each other's
/// records.
pub const SERVER_PAGE_MARKER: &str = "migrationhubstrategy:servers:";

/// The marker a `GetServerDetails` cursor carries.
///
/// That operation pages over ONE server's associated applications rather than over the
/// inventory, so it has its own marker: a `ListServers` token handed to it (or one of its
/// own handed to `ListServers`) is foreign and refused instead of resolving against the
/// other list's records.
pub const SERVER_DETAIL_PAGE_MARKER: &str = "migrationhubstrategy:server-applications:";

/// The marker a `ListAnalyzableServers` cursor carries.
///
/// That list holds the servers a collector fetched, which is a SUBSET of the inventory in
/// an order of its own, so it gets its own marker: a `ListServers` token handed to it (or
/// one of its own handed to `ListServers`) is foreign and refused instead of resuming at
/// whatever position its server happens to have in the other list.
pub const ANALYZABLE_SERVER_PAGE_MARKER: &str = "migrationhubstrategy:analyzable-servers:";

/// The marker a `ListCollectors` cursor carries.
///
/// That list pages over the installed collectors rather than over any list of servers, so
/// it gets its own marker: a token issued by one of this service's other lists is foreign
/// here (and one of these is foreign there) and refused instead of resolving against
/// another list's records.
pub const COLLECTOR_PAGE_MARKER: &str = "migrationhubstrategy:collectors:";

/// The marker a `ListApplicationComponents` cursor carries.
///
/// That list pages over the account's application components rather than over any list of
/// servers, so it gets its own marker: a token one of this service's other lists issued is
/// foreign here (and one of these is foreign there) and refused instead of resolving
/// against another list's records.
pub const APPLICATION_COMPONENT_PAGE_MARKER: &str =
    "migrationhubstrategy:application-components:";

/// Encode the key a page stopped at as an opaque `nextToken` carrying `marker`.
///
/// Hex round-trips exactly and is non-empty for a non-empty key, so the token satisfies
/// the `nextToken` members' `@length` and `.*\S.*`, and the next call resolves it against
/// the same map — no server-side cursor table. One codec for every list of this service,
/// so the token shape cannot drift between them; the marker is what tells the lists
/// apart.
pub fn encode_page_token(marker: &str, key: &str) -> String {
    hex::encode(format!("{marker}{key}").as_bytes())
}

/// The key an [`encode_page_token`] token carrying `marker` names; `None` for anything
/// this service did not mint, including well-formed hex carrying another list's marker.
pub fn decode_page_token(marker: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(marker).map(str::to_string)
}

/// What a `nextToken` of a FILTERED list of this service carries, beside the marker: the
/// list the page was taken from, and the record the next page starts at.
///
/// A token names a position in ONE list, so it carries a fingerprint of the SELECTION the
/// list was taken under ([`selection_fingerprint`]) — the criteria and value, the order and
/// the groups. Handed back with a different selection it is refused, rather than resuming
/// at whatever position that record happens to have in some other list. The page SIZE is
/// deliberately not part of it: `maxResults` is documented per call ("if a previous call to
/// this action returned 100 items, but you set maxResults to 10"), so changing it between
/// pages is the caller's business.
///
/// One cursor for every selected list of this service — the lists differ in what they hold
/// and not in how a page of them resumes — so the token shape cannot drift between them;
/// the marker is what tells the lists apart.
#[derive(Debug, Clone)]
pub struct ListCursor {
    /// The fingerprint of the selection the list was taken under.
    pub selection: String,
    /// The key of the first record the previous page did not report.
    pub resume_at: String,
}

impl ListCursor {
    /// This cursor as the opaque `nextToken` the caller hands back, through the service's
    /// one page-token codec under `marker`.
    pub fn to_token(&self, marker: &str) -> String {
        encode_page_token(marker, &format!("{}:{}", self.selection, self.resume_at))
    }

    /// The cursor a token carries, or `None` for anything the list owning `marker` did not
    /// mint.
    pub fn from_token(marker: &str, token: &str) -> Option<Self> {
        let payload = decode_page_token(marker, token)?;
        let (selection, resume_at) = payload.split_once(':')?;
        Some(Self {
            selection: selection.to_string(),
            resume_at: resume_at.to_string(),
        })
    }
}

/// A fingerprint of the selection a list was taken under: which records it holds and in
/// which order, independent of how they are paged.
///
/// A digest rather than the members themselves, so a token stays well inside `NextToken`'s
/// `@length(0, 2048)` however many groups the filter names — and so it carries nothing a
/// caller could read its own filter back out of.
pub fn selection_fingerprint(
    filter: Option<(&str, &str)>,
    descending: bool,
    groups: &[Group],
) -> String {
    use ::sha2::{Digest, Sha256};

    // The group filter is a SET: the same groups in another order select the same records,
    // so the fingerprint must not depend on the order they were sent in.
    let mut named: Vec<String> = groups
        .iter()
        .map(|group| {
            format!(
                "{:?}={:?}",
                group.name.as_deref().unwrap_or_default(),
                group.value.as_deref().unwrap_or_default()
            )
        })
        .collect();
    named.sort();
    let (criteria, value) = filter.unwrap_or((CRITERIA_NOT_DEFINED, ""));
    let order = if descending {
        SORT_DESCENDING
    } else {
        SORT_ASCENDING
    };
    let digest = <Sha256 as Digest>::digest(
        format!("{criteria:?}/{value:?}/{order}/{}", named.join(",")).as_bytes(),
    );
    // Half the digest: the fingerprint only has to tell two selections of one account's
    // portfolio apart, and a shorter token is a shorter `nextToken`.
    ::hex::encode(&digest[..16])
}

/// The page a list request asks for: how many records it takes, and the key it resumes
/// from.
#[derive(Debug, Clone)]
pub struct ListPage {
    /// How many records this page holds at most.
    pub size: usize,
    /// The key the page starts at, or `None` for a request that starts a list.
    pub resume_at: Option<String>,
}

/// Hold `maxResults` to the documented maximum, through the service's one [`page_size`]
/// decision, and `nextToken` to a token the list owning `marker` issued for THIS selection.
///
/// The refusals are this service's two `ValidationException` sentences — an unusable value
/// ([`invalid_request_body`], through [`page_size`]) and an unresolvable one
/// ([`page_token_not_accepted`]) — which each operation lifts into its own error channel.
pub fn list_page(
    max_results: Option<i32>,
    next_token: Option<String>,
    marker: &str,
    selection: &str,
) -> Result<ListPage, ValidationException> {
    let size = page_size(max_results)?;
    let resume_at = match next_token {
        None => None,
        Some(token) => {
            let cursor =
                ListCursor::from_token(marker, &token).ok_or_else(page_token_not_accepted)?;
            if cursor.selection != selection {
                return Err(page_token_not_accepted());
            }
            Some(cursor.resume_at)
        }
    };
    Ok(ListPage { size, resume_at })
}

/// The (criteria, value) pair a list request filters on, or `None` when it filters nothing.
///
/// A criteria other than [`CRITERIA_NOT_DEFINED`] names the member to compare, so it needs
/// a `filterValue` to compare against and is refused without one; `NOT_DEFINED` (and an
/// absent criteria) names no member, so it filters nothing and needs no value. Membership
/// in `declared` — the criteria enumeration's own generated value list — is checked here
/// because the boundary validator does not enforce it: a value the enumeration does not
/// declare is refused rather than quietly matching no record.
///
/// One rule for every criteria-filtered list of this service: each criteria enumeration
/// declares the same `NOT_DEFINED` member and each `filterValue` is documented the same way
/// ("Specify the value based on the … criteria type"), so the lists differ only in which
/// catalog they pass in and which member each value names.
pub fn filter_pair<'a>(
    criteria: Option<&'a str>,
    filter_value: Option<&'a str>,
    declared: &[&str],
) -> Result<Option<(&'a str, &'a str)>, ValidationException> {
    let criteria = criteria.unwrap_or(CRITERIA_NOT_DEFINED);
    if !declared.contains(&criteria) {
        return Err(invalid_request_body());
    }
    if criteria == CRITERIA_NOT_DEFINED {
        return Ok(None);
    }
    let value = filter_value.ok_or_else(invalid_request_body)?;
    Ok(Some((criteria, value)))
}

/// A counted number of servers or resources as this service's `Integer` members report it.
///
/// Narrowing a stored collection's length to the modelled width saturates rather than
/// wrapping, so a counter reports the maximum the member can carry instead of a negative
/// number.
pub fn count_of(counted: usize) -> i32 {
    i32::try_from(counted).unwrap_or(i32::MAX)
}

/// One assessment of an on-premises environment, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct Assessment {
    /// An `AsyncTaskId` minted by
    /// [`ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState::mint_assessment_id`].
    pub id: String,
    /// One of `AssessmentStatus`'s values.
    pub status: String,
    /// `StartAssessmentRequest$assessmentTargets` — the criteria of this assessment,
    /// reported back verbatim by `GetAssessment` and resolved into the scope below when
    /// the assessment started ([`RegionState::servers_covered_by`]).
    pub assessment_targets: Vec<AssessmentTarget>,
    pub assessment_data_source_type: Option<String>,
    pub s3bucket_for_analysis_data: Option<String>,
    pub s3bucket_for_report_data: Option<String>,
    pub start_time: SystemTime,
    pub completion_time: Option<SystemTime>,
    pub status_message: Option<String>,
    /// WHICH servers of the portfolio this assessment covers, by id: the ones its criteria
    /// selected out of the inventory the portfolio held when it started
    /// ([`RegionState::servers_covered_by`]).
    ///
    /// It is the assessment's scope, so it is what every counter it reports is counted off
    /// ([`Self::servers`]) and what the collection it runs and the analysis its completion
    /// records apply to — and to nothing else
    /// ([`RegionState::finish_assessment`]). Resolved once, at the start, because the
    /// counters report the assessment that was started rather than the portfolio as it is
    /// read.
    pub covered_servers: BTreeSet<String>,
    pub success: i32,
    pub failed: i32,
    pub in_progress: i32,
    /// The instant the clock owes this assessment `COMPLETE`, while it is running.
    pub completion_due: Option<SystemTime>,
}

impl Assessment {
    /// `DataCollectionDetails$servers` — "The total number of servers in the assessment".
    ///
    /// Counted off the scope itself, so the counter and the servers the assessment
    /// actually collects for cannot drift apart.
    pub fn servers(&self) -> i32 {
        count_of(self.covered_servers.len())
    }

    /// Materialise the completion the clock owes this assessment, if `now` is past its
    /// due instant: every server still in progress has finished collecting by then.
    ///
    /// Answers the instant THIS call materialised the completion at, which is what tells
    /// its caller the assessment's terminal status has to be materialised on the
    /// portfolio it covered as well ([`RegionState::finish_assessment`]) — and `None` for
    /// an assessment this call changed nothing about.
    pub fn settle(&mut self, now: SystemTime) -> Option<SystemTime> {
        let due = self.completion_due?;
        if now < due {
            return None;
        }
        self.status = ASSESSMENT_COMPLETE.to_string();
        self.completion_time = Some(due);
        self.success = self.success.saturating_add(self.in_progress);
        self.in_progress = 0;
        self.completion_due = None;
        self.status_message = Some(format!(
            "Data collection completed for {} of {} servers.",
            self.success,
            self.servers()
        ));
        Some(due)
    }

    /// True once this assessment reached a terminal `AssessmentStatus` — nothing is
    /// collecting any more, so there is nothing left to stop.
    pub fn is_terminal(&self) -> bool {
        self.completion_due.is_none()
    }

    /// Stop a running assessment: `STOPPED` is terminal, so the completion the clock
    /// owed it is cancelled and the servers that had not finished collecting are
    /// neither a success nor a failure.
    ///
    /// Answers `false` for an assessment that already reached a terminal status, whose
    /// stored status is left exactly as it was.
    pub fn stop(&mut self, now: SystemTime) -> bool {
        if self.is_terminal() {
            return false;
        }
        self.status = ASSESSMENT_STOPPED.to_string();
        self.completion_time = Some(now);
        self.status_message = Some(format!(
            "Data collection stopped with {} of {} servers collected.",
            self.success,
            self.servers()
        ));
        self.in_progress = 0;
        self.completion_due = None;
        true
    }

    /// The `DataCollectionDetails` `GetAssessment` reports: this record's status, its
    /// counters and its timestamps, computed from nothing else.
    pub fn to_data_collection_details(&self) -> DataCollectionDetails {
        DataCollectionDetails {
            status: Some(self.status.clone()),
            servers: Some(self.servers()),
            failed: Some(self.failed),
            success: Some(self.success),
            in_progress: Some(self.in_progress),
            start_time: Some(::aws_smithy_types::DateTime::from(self.start_time)),
            completion_time: self
                .completion_time
                .map(::aws_smithy_types::DateTime::from),
            status_message: self.status_message.clone(),
        }
    }
}

/// One recommendation report generation task, as this emulator persists it.
///
/// `StartRecommendationReportGeneration` mints it and `GetRecommendationReportDetails`
/// reports it; every member of the `RecommendationReportDetails` the read answers with is
/// computed from this record ([`Self::to_details`]).
#[derive(Debug, Clone)]
pub struct RecommendationReport {
    /// A `RecommendationTaskId` minted by
    /// [`ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState::mint_recommendation_report_id`].
    pub id: String,
    /// One of `RecommendationReportStatus`'s values.
    pub status: String,
    /// `RecommendationReportDetails$statusMessage`, set when the task reaches its
    /// terminal status.
    pub status_message: Option<String>,
    /// The `OutputFormat` the report was asked for, [`DEFAULT_OUTPUT_FORMAT`] when the
    /// request named none.
    pub output_format: String,
    /// How many servers of the portfolio the request's `groupIdFilter` selected when this
    /// task started — counted from the store, never from the request.
    ///
    /// The filter ITSELF is not kept: no member of `RecommendationReportDetails` reports
    /// it, so the record holds the count it produced rather than a caller-sized list for
    /// the life of the task.
    pub servers_covered: i32,
    pub start_time: SystemTime,
    pub completion_time: Option<SystemTime>,
    /// The instant the clock owes this task its terminal status, while it has one. It is
    /// also what makes the task "in progress": a start call made while one is owed is
    /// refused for this task rather than minting a second one.
    pub completion_due: Option<SystemTime>,
    /// `RecommendationReportDetails$s3Bucket` — "The S3 bucket where the report file is
    /// located": the account's report data bucket
    /// ([`RegionState::report_data_bucket`]), set by the start call once the report file is
    /// THERE. `None` when the account named no bucket for the reports this service
    /// generates, or when the object could not be written: a location is only ever held
    /// for an object a caller can fetch.
    pub s3_bucket: Option<String>,
    /// `RecommendationReportDetails$s3Keys` — "The Amazon S3 key name of the report
    /// file", the keys this task's report file was written under inside
    /// [`Self::s3_bucket`] ([`report_file::report_key`]). Empty for a task whose file was
    /// not written, and filled by the same call that fills the bucket, so the pair is
    /// either followable or absent.
    pub s3_keys: Vec<String>,
    /// Why this task has no report file to name, in the service's own words, for a task
    /// that could not produce one — the account named no report data bucket, or the object
    /// could not be written. `None` for a task whose file was written.
    pub no_file_reason: Option<String>,
}

impl RecommendationReport {
    /// Materialise the terminal status the clock owes this task, if `now` is past its due
    /// instant.
    ///
    /// `completion_time` is the DUE instant rather than `now`, so a task reports the same
    /// completion time however late it is first read. The status is [`REPORT_SUCCESS`],
    /// and the message reports what the task covered and what became of its report file
    /// ([`Self::terminal_message`]).
    pub fn settle(&mut self, now: SystemTime) {
        let Some(due) = self.completion_due else {
            return;
        };
        if now < due {
            return;
        }
        self.status = REPORT_SUCCESS.to_string();
        self.completion_time = Some(due);
        self.completion_due = None;
        self.status_message = Some(self.terminal_message());
    }

    /// `RecommendationReportDetails$statusMessage` for this task once it is terminal: how
    /// many servers of the portfolio it covered, and the report file it produced.
    ///
    /// Both halves come from the record — the scope the start call counted, and whether
    /// the file was written — and it carries nothing from the request that reads the task.
    /// `RecommendationReportStatusMessage` bounds the member at 512 characters, which
    /// every sentence below stays inside: the only interpolated values are a counted
    /// number and an `OutputFormat` value.
    fn terminal_message(&self) -> String {
        let scope = format!(
            "{} server(s) of the portfolio were in scope.",
            self.servers_covered
        );
        if self.file_location().is_some() {
            return format!(
                "{scope} The recommendation report was generated in {} format and uploaded to \
                 the report data bucket.",
                self.output_format
            );
        }
        match &self.no_file_reason {
            Some(reason) => format!("{scope} No recommendation report file was generated: {reason}"),
            None => format!("{scope} No recommendation report file was generated."),
        }
    }

    /// Where this task's report file is, once it is there: the bucket it was written into
    /// and the keys it was written under.
    ///
    /// The two are reported together or not at all, so no read can name a bucket without
    /// the key inside it that a caller would fetch.
    fn file_location(&self) -> Option<(String, Vec<String>)> {
        let bucket = self.s3_bucket.clone()?;
        if self.s3_keys.is_empty() {
            return None;
        }
        Some((bucket, self.s3_keys.clone()))
    }

    /// True while this task has not reached a terminal `RecommendationReportStatus`, so a
    /// new start call conflicts with it.
    pub fn is_in_progress(&self) -> bool {
        self.completion_due.is_some()
    }

    /// The `RecommendationReportDetails` `GetRecommendationReportDetails` reports: this
    /// record's status, message, timestamps and the location of the report file it wrote,
    /// and nothing derived from the request that reads it.
    ///
    /// `s3Bucket` and `s3Keys` name where the report FILE is, so they are reported with
    /// the other completion member: the file is the artifact of a FINISHED task, and a task
    /// that produced none names no location at all ([`REPORT_SUCCESS`]).
    pub fn to_details(&self) -> RecommendationReportDetails {
        let location = if self.is_in_progress() {
            None
        } else {
            self.file_location()
        };
        let (s3_bucket, s3_keys) = match location {
            Some((bucket, keys)) => (Some(bucket), Some(keys)),
            None => (None, None),
        };
        RecommendationReportDetails {
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
            start_time: Some(::aws_smithy_types::DateTime::from(self.start_time)),
            completion_time: self
                .completion_time
                .map(::aws_smithy_types::DateTime::from),
            s3_bucket,
            s3_keys,
        }
    }
}

/// The criteria value that names no member to filter on, so a list request carrying it (or
/// no criteria at all) filters nothing.
///
/// Each criteria enumeration of this service declares it — `ServerCriteria`
/// (`SERVER_CRITERIA_VALUES`) and `ApplicationComponentCriteria`
/// (`APPLICATION_COMPONENT_CRITERIA_VALUES`) — and in both every OTHER value names a member
/// of the record the list reports, so the one that names none is stated once here.
pub const CRITERIA_NOT_DEFINED: &str = "NOT_DEFINED";

/// The `ServerCriteria` value that names no member to filter on, so a request carrying it
/// (or no `serverCriteria` at all) filters nothing — the service-wide
/// [`CRITERIA_NOT_DEFINED`] under the name the server inventory reads it by.
pub const SERVER_CRITERIA_NOT_DEFINED: &str = CRITERIA_NOT_DEFINED;

/// The `ServerCriteria` value that names a server's OPERATING SYSTEM, which is the
/// attribute both the filter on it ([`ServerRecord::matches_criteria`]) and the operating
/// system a portfolio summary groups by ([`ServerRecord::summarised_os_type`]) resolve
/// through [`ServerRecord::attribute_values`].
pub const OS_NAME_ATTRIBUTE: &str = "OS_NAME";

/// `SortOrder`'s ascending value, which is the order a list is reported in when the
/// request names no `sort`.
pub const SORT_ASCENDING: &str = "ASC";

/// `SortOrder`'s descending value.
pub const SORT_DESCENDING: &str = "DESC";

/// `StrategyRecommendation`'s value for a strategy the service itself recommends: the
/// status the recommendation set stored on a server is reported with.
///
/// The other three values this enumeration declares (`viableOption`, `notRecommended`,
/// `potential`) rank a strategy some analysis considered and did not recommend. Nothing in
/// this emulator ranks strategies, so no strategy is reported with one of them, and a
/// strategy nothing recommended is reported with no status at all rather than with a rank
/// nothing computed.
pub const STRATEGY_RECOMMENDED: &str = "recommended";

/// `ServerOsType`'s value for an operating system the rest of that catalog
/// ([`SERVER_OS_TYPE_VALUES`](crate::ops::types::SERVER_OS_TYPE_VALUES)) does not name.
///
/// It is the catch-all of that enumeration, so it is what a portfolio summary groups a
/// server whose recorded operating system is none of the named ones under
/// ([`ServerRecord::summarised_os_type`]).
pub const SERVER_OS_TYPE_OTHER: &str = "Other";

/// `SrcCodeOrDbAnalysisStatus`'s value for an application component whose source code
/// and/or database credentials are configured and for which no analysis was asked for.
///
/// `UpdateApplicationComponentConfigRequest$configureOnly` is documented "If it is set to
/// true, the source code and/or database credentials are updated. If it is set to false,
/// the source code and/or database credentials are updated and an analysis is initiated",
/// so a configure-only update leaves the component configured and unanalysed.
pub const ANALYSIS_CONFIGURED: &str = "CONFIGURED";

/// `SrcCodeOrDbAnalysisStatus`'s value for an application component whose analysis has
/// been asked for and not yet scheduled.
///
/// It is where an update that asked for an analysis (the same documented sentence, with
/// `configureOnly` false or absent) leaves the component, and where it STAYS: nothing in
/// this emulator analyses source code or a database, so no analyzer picks the queued
/// analysis up and no result, success or failure is ever reported for work nothing ran.
pub const ANALYSIS_TO_BE_SCHEDULED: &str = "ANALYSIS_TO_BE_SCHEDULED";

/// The `RecommendationSet` a stored strategy, destination and transformation tool amount
/// to, or `None` when none of the three is held.
///
/// The three are what an analysis stored on the resource ([`analysis::Recommendation`]) —
/// the tool being named by its name alone, with no description and no installation link,
/// because what the analysis picked is a catalog entry rather than a catalog. Both the
/// server inventory and the application components hold those three members, so both
/// report their recommendation through this one function.
fn recommendation_of(
    strategy: &Option<String>,
    target_destination: &Option<String>,
    transformation_tool: &Option<String>,
) -> Option<RecommendationSet> {
    (strategy.is_some() || target_destination.is_some() || transformation_tool.is_some()).then(
        || RecommendationSet {
            transformation_tool: transformation_tool.clone().map(|name| TransformationTool {
                name: Some(name),
                description: None,
                tranformation_tool_installation_link: None,
            }),
            target_destination: target_destination.clone(),
            strategy: strategy.clone(),
        },
    )
}

/// How many resources hold each value of ONE of their attributes: every value at least
/// one of them holds, paired with the number holding it, in the values' own order.
///
/// It is the one counting rule behind every summary this service reports — a strategy
/// summary, a count per operating system, per application type or per analysis status —
/// so two summaries over the same records cannot count them differently. A resource
/// holding NO value for the attribute counts towards no group, which is what keeps a
/// count from standing for a value nothing recorded, and a value nothing holds gets no
/// group at all rather than a zero.
fn counted_by_value<'a>(values: impl Iterator<Item = Option<&'a str>>) -> Vec<(String, i32)> {
    let mut counted: BTreeMap<&str, usize> = BTreeMap::new();
    for value in values.flatten() {
        *counted.entry(value).or_default() += 1;
    }
    counted
        .into_iter()
        .map(|(value, holding)| (value.to_string(), count_of(holding)))
        .collect()
}

/// The `StrategySummary` list a group of resources amounts to — "the summary of the
/// strategy recommendations", whose `count` is "the count of recommendations per
/// strategy".
///
/// Counted off the resources' own recommended strategies ([`counted_by_value`]), so a
/// summary and the resources it summarises cannot disagree, and a resource the analysis
/// recommended nothing for counts towards no strategy.
fn strategy_summary<'a>(strategies: impl Iterator<Item = Option<&'a str>>) -> Vec<StrategySummary> {
    counted_by_value(strategies)
        .into_iter()
        .map(|(strategy, count)| StrategySummary {
            strategy: Some(strategy),
            count: Some(count),
        })
        .collect()
}

/// One operating-system name as it is COMPARED against the `ServerOsType` catalog:
/// lower-cased, with everything that is not a letter or a digit dropped.
///
/// A server's operating system is free text (`SystemInfo$osInfo$version`), so the same
/// system is written in many ways; folding away case, spaces and punctuation is what makes
/// the one written "Amazon Linux 2" and the one written "AmazonLinux2" the same
/// `ServerOsType`.
fn fold_os_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_alphanumeric())
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

/// The `StrategySummary` list a server reports for the application components running on
/// it — `ServerDetail$applicationComponentStrategySummary`, "a list of strategy
/// summaries", whose `count` is "the count of recommendations per strategy".
///
/// It is counted off the component records themselves ([`strategy_summary`]), grouped by
/// the strategy the analysis recommended for each one, so the summary and the components
/// cannot disagree. A component the analysis recommended nothing for counts towards no
/// strategy, and one kept out of the assessment counts towards nothing at all:
/// `inclusionStatus` "indicates whether the application component has been included for
/// server recommendation or not". The groups come out in the strategies' own order, and a
/// server nothing runs on gets an EMPTY list, which its read reports as no summary at all.
fn component_strategy_summary(
    server_id: &str,
    components: &BTreeMap<String, ApplicationComponentRecord>,
) -> Vec<StrategySummary> {
    strategy_summary(
        components
            .values()
            .filter(|component| component.recommended_for_server(server_id))
            .map(|component| component.strategy.as_deref()),
    )
}

/// How many application components running on the server `server_id` the analysis
/// recommended `strategy` for — `ServerStrategy$numberOfApplicationComponents`, "the
/// number of application components with this strategy recommendation running on the
/// server".
///
/// Counted off the same records as [`component_strategy_summary`], so one number is the
/// other's entry: a strategy none of the server's components carries counts zero. A server
/// this service holds no component for has nothing to count, so the member is reported as
/// absent rather than as a zero no component stands behind; so is a listing that names no
/// strategy at all, which is not a strategy recommendation to count components for.
fn components_with_strategy(
    server_id: &str,
    strategy: Option<&str>,
    components: &BTreeMap<String, ApplicationComponentRecord>,
) -> Option<i32> {
    let strategy = strategy?;
    let mut running = components
        .values()
        .filter(|component| component.recommended_for_server(server_id))
        .peekable();
    running.peek()?;
    let counted = running
        .filter(|component| component.strategy.as_deref() == Some(strategy))
        .count();
    Some(count_of(counted))
}

/// The `RecommendationSet` a stored preference names.
///
/// `StrategyOption` and `RecommendationSet` carry the same three things under different
/// names — a strategy, a destination and a tool — so the preference is reported as the
/// recommendation it selects, its `toolName` becoming the `TransformationTool` the option
/// named and nothing more (no description and no installation link: the caller named a
/// tool, not a tool catalog entry).
fn recommendation_of_option(option: &StrategyOption) -> RecommendationSet {
    RecommendationSet {
        transformation_tool: option.tool_name.clone().map(|name| TransformationTool {
            name: Some(name),
            description: None,
            tranformation_tool_installation_link: None,
        }),
        target_destination: option.target_destination.clone(),
        strategy: option.strategy.clone(),
    }
}

/// One entry of the strategy list a read reports for a resource of this service.
///
/// `ServerStrategy` and `ApplicationComponentStrategy` declare the same three members (the
/// server's one also counts the application components running the strategy), so the list
/// is decided once here and each read maps it into its own shape.
struct StrategyListing {
    recommendation: RecommendationSet,
    status: Option<String>,
    is_preferred: bool,
}

/// The strategies a read reports for a resource holding the recommendation `analyzed` and
/// the preference `preferred`: the option the account prefers first, then the service's
/// own recommendation when it is a different one.
///
/// Both come from the resource's own record, so one nothing recommended and nobody chose a
/// preference for reports NO strategy rather than a canned list. `isPreferred` is "set to
/// true if the recommendation is set as preferred", so it is true exactly for the option an
/// update stored (the flag that call carried when it carried one) and false for a
/// recommendation nobody chose. A preference naming the same strategy and destination as
/// the service's recommendation is ONE strategy, not two, and is reported once — carrying
/// the recommended status, since it is also what the service recommends.
fn strategy_listings(
    analyzed: Option<RecommendationSet>,
    preferred: Option<&StrategyOption>,
) -> Vec<StrategyListing> {
    let preferred_set = preferred.map(recommendation_of_option);
    let same_strategy = |left: &RecommendationSet, right: &RecommendationSet| {
        left.strategy == right.strategy && left.target_destination == right.target_destination
    };
    let also_recommended = match (&analyzed, &preferred_set) {
        (Some(analyzed), Some(preferred)) => same_strategy(analyzed, preferred),
        _ => false,
    };
    let mut listings: Vec<StrategyListing> = Vec::new();
    if let Some(recommendation) = preferred_set {
        listings.push(StrategyListing {
            recommendation,
            status: also_recommended.then(|| STRATEGY_RECOMMENDED.to_string()),
            is_preferred: preferred
                .and_then(|option| option.is_preferred)
                .unwrap_or(true),
        });
    }
    if let Some(recommendation) = analyzed.filter(|_| !also_recommended) {
        listings.push(StrategyListing {
            recommendation,
            status: Some(STRATEGY_RECOMMENDED.to_string()),
            is_preferred: false,
        });
    }
    listings
}

/// What a data collector fetched about one server when it discovered it — the
/// `AnalyzableServerSummary` members, held on the server they describe.
///
/// `ListAnalyzableServers` is documented "Retrieves a list of all the servers fetched from
/// customer vCenter using Strategy Recommendation Collector", so an analyzable server is a
/// server the service holds a collector's discovery data for. That data is held HERE,
/// beside the rest of the server's record ([`ServerRecord::discovery`]), rather than in a
/// second collection of servers: a discovered server and a portfolio server are one
/// machine, and `AnalyzableServerSummary` carries no id of its own that could key a
/// separate one.
///
/// Every member is what the discovery stored, so a read derives nothing from the request
/// that reads it, and a member the discovery did not carry stays `None` rather than being
/// filled with a value nothing fetched. None of these four members is a member of
/// `ServerDetail`, which is why `ListServers` reports none of them: the model gives the
/// server's addresses to `SystemInfo$networkInfoList`, whose `NetworkInfo` requires an
/// interface name, a MAC address and a subnet mask that no part of this emulator holds.
#[derive(Debug, Clone, Default)]
pub struct ServerDiscovery {
    /// `AnalyzableServerSummary$hostname` — "The host name of the analyzable server."
    pub hostname: Option<String>,
    /// `AnalyzableServerSummary$ipAddress` — "The ip address of the analyzable server."
    pub ip_address: Option<String>,
    /// `AnalyzableServerSummary$source` — "The data source of the analyzable server."
    ///
    /// A free-text string in the model (it targets `String`, not `DataSourceType`), so it
    /// is held as the discovery recorded it.
    pub source: Option<String>,
    /// `AnalyzableServerSummary$vmId` — "The virtual machine id of the analyzable server."
    pub vm_id: Option<String>,
}

/// One server of this account's portfolio, as this emulator persists it.
///
/// A server enters the portfolio when an import file task ingests it from its import file
/// ([`import_file`]), which is why a freshly booted emulator holds no server at all. This
/// record is what the read path computes from: `ListServers` reports it as a
/// [`ServerDetail`], filters it by the member a `ServerCriteria` names, and scopes it by
/// the import groups it came in with.
///
/// Every member is what the import stored, so nothing here is derived from the request
/// that reads it.
#[derive(Debug, Clone, Default)]
pub struct ServerRecord {
    /// `ServerDetail$id` — a `ResourceId` (`^[0-9a-b]+`, up to 44 characters). It is also
    /// this server's key in [`RegionState::servers`],
    /// which is what gives a list a stable total order to sort and paginate over.
    pub id: String,
    /// The resource id the import file DECLARED for this server, which is what the
    /// entries of an import file refer to a server by ([`import_file`]).
    ///
    /// Not a member of `ServerDetail`, and not the id the service reports: it is the
    /// account's own name for the machine, so an application component declaring that it
    /// runs on that server resolves to this record whichever import brought the server
    /// in. `None` for a server whose entry declared only a name.
    pub external_id: Option<String>,
    /// `ServerDetail$name`.
    pub name: Option<String>,
    /// `ServerDetail$serverType`.
    pub server_type: Option<String>,
    /// `SystemInfo$osInfo$type` — one of `OSType`'s values.
    pub os_type: Option<String>,
    /// `SystemInfo$osInfo$version` — the free-text operating system the server reports.
    pub os_version: Option<String>,
    /// `SystemInfo$fileSystemType`.
    pub file_system_type: Option<String>,
    /// `SystemInfo$cpuArchitecture`.
    pub cpu_architecture: Option<String>,
    /// `ServerDetail$dataCollectionStatus` — one of `RunTimeAssessmentStatus`'s values.
    ///
    /// Documented "The status of assessment for the server", so it is the per-server face
    /// of the assessment covering this server: `StartAssessment` puts it in
    /// [`COLLECTION_STARTED`] ([`RegionState::begin_server_collection`]) and the terminal
    /// status the assessment reaches moves it to [`COLLECTION_STOPPED`] or
    /// [`COLLECTION_SUCCESS`] ([`RegionState::finish_server_collection`]). `None` for a
    /// server no assessment ever covered.
    pub data_collection_status: Option<String>,
    /// `ServerDetail$statusMessage`.
    pub status_message: Option<String>,
    /// `RecommendationSet$strategy` — one of `Strategy`'s values, once an analysis
    /// recommended one.
    pub strategy: Option<String>,
    /// `RecommendationSet$targetDestination` — one of `TargetDestination`'s values.
    pub target_destination: Option<String>,
    /// `TransformationTool$name` of the recommendation held for this server — one of
    /// `TransformationToolName`'s values, once an analysis picked the tool that performs
    /// the move it recommends ([`analysis::Recommendation`]). `None` where the catalog
    /// names no tool for that move.
    pub transformation_tool: Option<String>,
    /// `ServerError$serverErrorCategory` — one of `ServerErrorCategory`'s values, for a
    /// server whose analysis failed.
    pub error_category: Option<String>,
    /// `ServerDetail$lastAnalyzedTimestamp` — "the timestamp of when the server was
    /// assessed", which is the instant the assessment covering it reached `COMPLETE` and
    /// the analysis above was recorded ([`RegionState::analyze_covered_resources`]).
    pub last_analyzed: Option<SystemTime>,
    /// The import groups this server came in with — `StartImportFileTaskRequest$groupId`
    /// of the import that brought it in. Not a member of `ServerDetail`: it is what
    /// `ListServers`'s `groupIdFilter` scopes a list by.
    pub group_id: Vec<Group>,
    /// The strategy option the account PREFERS for this server, as `UpdateServerConfig`
    /// stored it — member for member, so nothing here is derived from the request that
    /// reads it. `None` until a caller sets one.
    ///
    /// It records the CUSTOMER's choice rather than any analysis, so it is held beside the
    /// recommendation members above instead of overwriting them: `GetServerStrategies`
    /// reports both, and `ServerStrategy$isPreferred` is the member the model gives the
    /// distinction to.
    pub preferred_strategy: Option<StrategyOption>,
    /// The applications this server belongs to — `AssociatedApplication` is "Object
    /// containing details about applications as defined in Application Discovery Service"
    /// — as the import that brought the server in stored them, in the order it stored
    /// them.
    ///
    /// Not a member of `ServerDetail`: it is the list `GetServerDetails` reports BESIDE
    /// the detail, and the one that operation pages over. Empty for a server no
    /// application was recorded for, so that read answers an empty list rather than an
    /// application nothing holds.
    pub associated_applications: Vec<AssociatedApplication>,
    /// The discovery data a collector fetched for this server ([`ServerDiscovery`]), or
    /// `None` for a server no collector fetched — a server that came in some other way
    /// (an import file, Application Discovery Service) and that the service therefore
    /// holds no vCenter discovery data for.
    ///
    /// It is this member that makes a server ANALYZABLE: `ListAnalyzableServers` reports
    /// the servers that carry it ([`Self::to_analyzable_summary`]) and no others, so a
    /// server nothing discovered is not reported as an empty summary.
    pub discovery: Option<ServerDiscovery>,
}

impl ServerRecord {
    /// The `ServerDetail` a read reports for this server: this record's own members, the
    /// components of `components` running on it, and nothing for the ones no part of this
    /// emulator produces.
    ///
    /// `listAntipatternSeveritySummary` and `antipatternReport*` are left unset rather
    /// than filled with a shape nothing computed — no anti-pattern report is generated —
    /// and the nested structures are reported only when the record holds something to put
    /// in them, so a server the import knew nothing about does not come back as a tree of
    /// empty objects. `applicationComponentStrategySummary` is counted off the components
    /// themselves ([`component_strategy_summary`]).
    pub fn to_server_detail(
        &self,
        components: &BTreeMap<String, ApplicationComponentRecord>,
    ) -> ServerDetail {
        let os_info = (self.os_type.is_some() || self.os_version.is_some()).then(|| OSInfo {
            r#type: self.os_type.clone(),
            version: self.os_version.clone(),
        });
        let system_info = (os_info.is_some()
            || self.file_system_type.is_some()
            || self.cpu_architecture.is_some())
        .then(|| SystemInfo {
            os_info,
            file_system_type: self.file_system_type.clone(),
            // The import brings no interface, address or mask with it, so there is no
            // network information to report.
            network_info_list: None,
            cpu_architecture: self.cpu_architecture.clone(),
        });
        let recommendation_set = self.analyzed_recommendation();
        let component_strategies = component_strategy_summary(&self.id, components);
        ServerDetail {
            id: Some(self.id.clone()),
            name: self.name.clone(),
            recommendation_set,
            data_collection_status: self.data_collection_status.clone(),
            status_message: self.status_message.clone(),
            list_antipattern_severity_summary: None,
            system_info,
            application_component_strategy_summary: (!component_strategies.is_empty())
                .then_some(component_strategies),
            antipattern_report_s3_object: None,
            antipattern_report_status: None,
            antipattern_report_status_message: None,
            server_type: self.server_type.clone(),
            last_analyzed_timestamp: self.last_analyzed.map(::aws_smithy_types::DateTime::from),
            server_error: self.error_category.clone().map(|category| ServerError {
                server_error_category: Some(category),
            }),
        }
    }

    /// True when this server satisfies `criteria` = `value`.
    ///
    /// Each `ServerCriteria` value names ONE member of the server record and the value is
    /// compared to it for equality — `filterValue` is documented as "the filter value,
    /// which is based on the type of server criteria … if `serverCriteria` is `OS_NAME`,
    /// and the `filterValue` is equal to `WindowsServer`, then `ListServers` returns all
    /// of the servers matching the OS name `WindowsServer`". `OS_NAME` is the one criteria
    /// whose member the model splits in two — `SystemInfo$osInfo` carries an `OSType` and
    /// a free-text version — so a value equal to either selects the server.
    ///
    /// [`SERVER_CRITERIA_NOT_DEFINED`] names no member, so it selects every server. A
    /// value `ServerCriteria` does not declare selects none; a caller never sees that,
    /// because the handler refuses an undeclared criteria before it gets here.
    pub fn matches_criteria(&self, criteria: &str, value: &str) -> bool {
        criteria == SERVER_CRITERIA_NOT_DEFINED || self.attribute_values(criteria).contains(&value)
    }

    /// EVERY value this server holds for the attribute `name` names, in the names
    /// `ServerCriteria` gives this service's server attributes.
    ///
    /// One value per attribute, except the one the model splits in two: `OS_NAME` is
    /// carried by `SystemInfo$osInfo`'s `OSType` and by its free-text version, so both are
    /// values of that attribute. An attribute the record holds nothing for, and a `name`
    /// that is not one of the catalog's, yield no value at all \u2014 which is what makes a
    /// criterion over them select nothing.
    ///
    /// It is the one place a criteria name is resolved to the member it names, so the
    /// filter a list applies ([`Self::matches_criteria`]) and the criteria an assessment
    /// is scoped by ([`Self::matches_target`]) cannot read different members for the same
    /// name.
    fn attribute_values(&self, name: &str) -> Vec<&str> {
        match name {
            "SERVER_ID" => vec![self.id.as_str()],
            OS_NAME_ATTRIBUTE => [self.os_type.as_deref(), self.os_version.as_deref()]
                .into_iter()
                .flatten()
                .collect(),
            "STRATEGY" => self.strategy.as_deref().into_iter().collect(),
            "DESTINATION" => self.target_destination.as_deref().into_iter().collect(),
            "ANALYSIS_STATUS" => self.data_collection_status.as_deref().into_iter().collect(),
            "ERROR_CATEGORY" => self.error_category.as_deref().into_iter().collect(),
            _ => Vec::new(),
        }
    }

    /// True when this server satisfies one criterion of an assessment.
    ///
    /// `AssessmentTarget` is documented "Defines the criteria of assessment" and carries a
    /// `name`, the `values` to compare against and the `condition` to compare them under,
    /// so the criterion compares the values of the attribute `name` names
    /// ([`Self::attribute_values`]) to the values it lists: `EQUALS` holds when a held
    /// value IS one of them and `CONTAINS` when a held value contains one of them as
    /// text, and the two negated conditions are exactly their complements \u2014 so a server
    /// holding no value for the attribute satisfies `NOT_EQUALS` and `NOT_CONTAINS` and
    /// fails the other two. A `condition` outside the `Condition` catalog compares
    /// nothing, so it selects no server; a caller never sees that, because the handler
    /// refuses an undeclared condition before anything is stored.
    pub fn matches_target(&self, target: &AssessmentTarget) -> bool {
        let held = self.attribute_values(&target.name);
        let matched = |compare: fn(&str, &str) -> bool| -> bool {
            held.iter()
                .any(|value| target.values.iter().any(|wanted| compare(value, wanted)))
        };
        match target.condition.as_str() {
            "EQUALS" => matched(|value, wanted| value == wanted),
            "NOT_EQUALS" => !matched(|value, wanted| value == wanted),
            "CONTAINS" => matched(|value, wanted| value.contains(wanted)),
            "NOT_CONTAINS" => !matched(|value, wanted| value.contains(wanted)),
            _ => false,
        }
    }

    /// The `RecommendationSet` the service's own recommendation for this server amounts
    /// to, or `None` when it holds none.
    ///
    /// The same three members `to_server_detail` reports: the strategy, the destination
    /// and the transformation tool the analysis recorded.
    fn analyzed_recommendation(&self) -> Option<RecommendationSet> {
        recommendation_of(
            &self.strategy,
            &self.target_destination,
            &self.transformation_tool,
        )
    }

    /// The strategies a read reports for this server — [`strategy_listings`] as
    /// `ServerStrategy`.
    ///
    /// `numberOfApplicationComponents` is counted off the components of `components`
    /// running on this server that carry the listing's own strategy
    /// ([`components_with_strategy`]).
    pub fn to_server_strategies(
        &self,
        components: &BTreeMap<String, ApplicationComponentRecord>,
    ) -> Vec<ServerStrategy> {
        strategy_listings(
            self.analyzed_recommendation(),
            self.preferred_strategy.as_ref(),
        )
        .into_iter()
        .map(|listing| ServerStrategy {
            number_of_application_components: components_with_strategy(
                &self.id,
                listing.recommendation.strategy.as_deref(),
                components,
            ),
            recommendation: Some(listing.recommendation),
            status: listing.status,
            is_preferred: Some(listing.is_preferred),
        })
        .collect()
    }

    /// The `AnalyzableServerSummary` a read reports for this server, or `None` when no
    /// collector discovered it and the service therefore holds nothing to summarise.
    ///
    /// The four members of the stored [`ServerDiscovery`], member for member and nothing
    /// else: `AnalyzableServerSummary` declares exactly those four, so a summary carries
    /// what the discovery fetched and leaves absent what it did not.
    pub fn to_analyzable_summary(&self) -> Option<AnalyzableServerSummary> {
        let discovery = self.discovery.as_ref()?;
        Some(AnalyzableServerSummary {
            hostname: discovery.hostname.clone(),
            ip_address: discovery.ip_address.clone(),
            source: discovery.source.clone(),
            vm_id: discovery.vm_id.clone(),
        })
    }

    /// True when this server came in with one of `groups` — the service's one
    /// [`in_any_group`], over the groups this record holds.
    pub fn in_any_group(&self, groups: &[Group]) -> bool {
        in_any_group(&self.group_id, groups)
    }

    /// The `ServerOsType` a portfolio summary groups this server under —
    /// `ServerSummary$ServerOsType`, "type of operating system for the servers" — or
    /// `None` when the service holds no operating system for it at all, so it counts
    /// towards no group rather than towards one nothing recorded.
    ///
    /// `ServerOsType` is a CATALOG of operating systems with a catch-all
    /// ([`SERVER_OS_TYPE_OTHER`]), while a server's own record holds its `OSType` and the
    /// free-text system it reports: so the server is grouped under the catalog value its
    /// recorded operating system NAMES, compared with case, spaces and punctuation folded
    /// away ([`fold_os_name`]), and under the catch-all when what it records is none of
    /// them. The longest catalog value a record names wins, because one of them contains
    /// another as text: a server reporting an end-of-support Windows Server is not grouped
    /// with the supported ones.
    ///
    /// The operating system is read through the one place that resolves an attribute name
    /// to the members holding it ([`Self::attribute_values`]), so the type a summary groups
    /// by and the member `ListServers` filters on cannot come from different members.
    pub fn summarised_os_type(&self) -> Option<&'static str> {
        let held: Vec<String> = self
            .attribute_values(OS_NAME_ATTRIBUTE)
            .into_iter()
            .map(fold_os_name)
            .collect();
        if held.is_empty() {
            return None;
        }
        let mut catalog: Vec<&'static str> = SERVER_OS_TYPE_VALUES
            .iter()
            .copied()
            .filter(|value| *value != SERVER_OS_TYPE_OTHER)
            .collect();
        catalog.sort_by_key(|value| ::std::cmp::Reverse(value.len()));
        let named = catalog.into_iter().find(|value| {
            let wanted = fold_os_name(value);
            held.iter().any(|held| held.contains(&wanted))
        });
        Some(named.unwrap_or(SERVER_OS_TYPE_OTHER))
    }
}

/// True when a resource that came in with `held` belongs to one of the groups `wanted`
/// names.
///
/// A `Group` is a (`name`, `value`) pair — "The object containing information about
/// distinct imports or groups" — so a resource is in a filter's group only when both halves
/// agree, and several groups in a filter widen the selection rather than narrowing it: a
/// `groupIdFilter` names the groups whose resources to report.
///
/// An EMPTY `wanted` answers `false`, which is why a caller that named no group at all must
/// not reach here: naming no group scopes nothing, so a handler applies this only to a
/// non-empty filter. One rule for every list of this service that takes a `groupIdFilter`,
/// so a server and an application component are scoped by a filter the same way.
fn in_any_group(held: &[Group], wanted: &[Group]) -> bool {
    wanted.iter().any(|wanted| {
        held.iter()
            .any(|held| held.name == wanted.name && held.value == wanted.value)
    })
}

/// One application component of this account's portfolio, as this emulator persists it.
///
/// An application component is "the process or database running on a server" that the
/// portfolio holds a recommendation for, so it is a resource of its own beside
/// [`ServerRecord`] rather than a member of it. Like a server it enters this service
/// through the data the account brings in (an import file task's ingestion,
/// [`import_file`]), which is why a freshly booted emulator holds no
/// application component at all.
///
/// Every member below is what that ingestion stored, so nothing in a read is derived from
/// the request that reads it: `GetApplicationComponentDetails` reports this record as the
/// `ApplicationComponentDetail` the model declares ([`Self::to_detail`]) beside the
/// associations it holds.
#[derive(Debug, Clone, Default)]
pub struct ApplicationComponentRecord {
    /// `ApplicationComponentDetail$id` — a `ResourceId` (`^[0-9a-b]+`, up to 44
    /// characters). It is also this component's key in
    /// [`RegionState::application_components`],
    /// which is what gives a list over them a stable total order.
    pub id: String,
    /// `ApplicationComponentDetail$name`.
    pub name: Option<String>,
    /// `ApplicationComponentDetail$appType` — one of `AppType`'s values
    /// ([`APP_TYPE_VALUES`](crate::ops::types::APP_TYPE_VALUES)).
    pub app_type: Option<String>,
    /// `ApplicationComponentDetail$resourceSubType` — one of `ResourceSubType`'s values.
    pub resource_sub_type: Option<String>,
    /// `ApplicationComponentDetail$inclusionStatus` — one of `InclusionStatus`'s values:
    /// whether the component is included in server recommendation.
    pub inclusion_status: Option<String>,
    /// `ApplicationComponentDetail$analysisStatus` — one of
    /// `SrcCodeOrDbAnalysisStatus`'s values, for a component with source code or a
    /// database attached.
    pub analysis_status: Option<String>,
    /// `ApplicationComponentDetail$statusMessage`.
    pub status_message: Option<String>,
    /// `RecommendationSet$strategy` — one of `Strategy`'s values, once an analysis
    /// recommended one for this component.
    pub strategy: Option<String>,
    /// `RecommendationSet$targetDestination` — one of `TargetDestination`'s values.
    pub target_destination: Option<String>,
    /// `TransformationTool$name` of the recommendation held for this component — one of
    /// `TransformationToolName`'s values, once an analysis picked the tool that performs
    /// the move it recommends ([`analysis::Recommendation`]).
    pub transformation_tool: Option<String>,
    /// `ApplicationComponentDetail$osVersion`.
    pub os_version: Option<String>,
    /// `ApplicationComponentDetail$osDriver`.
    pub os_driver: Option<String>,
    /// `ApplicationComponentDetail$lastAnalyzedTimestamp` — the instant the assessment
    /// covering this component reached `COMPLETE` and the analysis above was recorded
    /// ([`RegionState::analyze_covered_resources`]).
    pub last_analyzed: Option<SystemTime>,
    /// `DatabaseConfigDetail$secretName` — the Secrets Manager secret the component's
    /// database is reached with, for a component that has one.
    pub database_secret_name: Option<String>,
    /// `ApplicationComponentDetail$sourceCodeRepositories`, in the order they were
    /// stored. Empty for a component no repository was recorded for.
    pub source_code_repositories: Vec<SourceCodeRepository>,
    /// `ApplicationComponentDetail$runtimeStatus` — one of `RuntimeAnalysisStatus`'s
    /// values.
    pub runtime_status: Option<String>,
    /// `ApplicationComponentDetail$runtimeStatusMessage`.
    pub runtime_status_message: Option<String>,
    /// `AppUnitError$appUnitErrorCategory` — one of `AppUnitErrorCategory`'s values, for
    /// a component whose source-code or database analysis failed.
    pub app_unit_error_category: Option<String>,
    /// The ids of the servers this component runs on, in the order they were stored —
    /// `GetApplicationComponentDetailsResponse$associatedServerIds`, "a list of the IDs of
    /// the servers on which the application component is running".
    ///
    /// The detail's own `associatedServerId` ("the ID of the server that the application
    /// component is running on") is the first of them and
    /// `moreServerAssociationExists` is whether there is more than one, so the single
    /// member and the list can never disagree. Empty for a component no server
    /// association was recorded for.
    pub associated_server_ids: Vec<String>,
    /// The application groups this component belongs to, as defined in Application
    /// Discovery Service, in the order they were stored —
    /// `GetApplicationComponentDetailsResponse$associatedApplications`.
    ///
    /// `moreApplicationResource` ("set to true if the application component belongs to
    /// more than one application group") is whether there is more than one, counted off
    /// this list.
    pub associated_applications: Vec<AssociatedApplication>,
    /// The strategy option the account PREFERS for this component, as
    /// `UpdateApplicationComponentConfig` stored it — member for member, so nothing here
    /// is derived from the request that reads it. `None` until a caller sets one.
    ///
    /// Like the server inventory's preference ([`ServerRecord::preferred_strategy`]) it
    /// records the CUSTOMER's choice rather than any analysis, so it is held beside the
    /// recommendation members above instead of overwriting them:
    /// `GetApplicationComponentStrategies` reports both, and
    /// `ApplicationComponentStrategy$isPreferred` is the member the model gives the
    /// distinction to.
    pub preferred_strategy: Option<StrategyOption>,
    /// The import groups this component came in with —
    /// `StartImportFileTaskRequest$groupId` of the import that brought it in, whose
    /// documentation is "This ID can be as filter in ListApplicationComponents and
    /// ListServers".
    ///
    /// Not a member of `ApplicationComponentDetail`: like the server inventory's own groups
    /// ([`ServerRecord::group_id`]) it is what a list's `groupIdFilter` scopes the answer
    /// by. Empty for a component that came in with no group.
    pub group_id: Vec<Group>,
}

impl ApplicationComponentRecord {
    /// The `ApplicationComponentDetail` a read reports for this component: this record's
    /// own members, and nothing for the ones no part of this emulator produces.
    ///
    /// `listAntipatternSeveritySummary` and `antipatternReport*` are left unset rather
    /// than filled with a shape nothing computed — this emulator generates no
    /// anti-pattern report — and so is `resultList`, the list of analysis results, since
    /// nothing here analyses source code or a database. The nested structures are
    /// reported only when the record holds something to put in them, so a component the
    /// ingestion knew little about does not come back as a tree of empty objects.
    pub fn to_detail(&self) -> ApplicationComponentDetail {
        let recommendation_set = self.analyzed_recommendation();
        ApplicationComponentDetail {
            id: Some(self.id.clone()),
            name: self.name.clone(),
            recommendation_set,
            analysis_status: self.analysis_status.clone(),
            status_message: self.status_message.clone(),
            list_antipattern_severity_summary: None,
            database_config_detail: self.database_secret_name.clone().map(|secret_name| {
                DatabaseConfigDetail {
                    secret_name: Some(secret_name),
                }
            }),
            source_code_repositories: (!self.source_code_repositories.is_empty())
                .then(|| self.source_code_repositories.clone()),
            app_type: self.app_type.clone(),
            resource_sub_type: self.resource_sub_type.clone(),
            inclusion_status: self.inclusion_status.clone(),
            antipattern_report_s3_object: None,
            antipattern_report_status: None,
            antipattern_report_status_message: None,
            os_version: self.os_version.clone(),
            os_driver: self.os_driver.clone(),
            last_analyzed_timestamp: self.last_analyzed.map(::aws_smithy_types::DateTime::from),
            associated_server_id: self.associated_server_ids.first().cloned(),
            more_server_association_exists: Some(self.associated_server_ids.len() > 1),
            runtime_status: self.runtime_status.clone(),
            runtime_status_message: self.runtime_status_message.clone(),
            app_unit_error: self
                .app_unit_error_category
                .clone()
                .map(|category| AppUnitError {
                    app_unit_error_category: Some(category),
                }),
            result_list: None,
        }
    }

    /// Whether this component belongs to more than one application group —
    /// `GetApplicationComponentDetailsResponse$moreApplicationResource`.
    ///
    /// Counted off [`Self::associated_applications`], the very list the same answer
    /// reports, so the flag and the list cannot drift apart.
    pub fn more_application_resource(&self) -> bool {
        self.associated_applications.len() > 1
    }

    /// The `RecommendationSet` the service's own recommendation for this component amounts
    /// to, or `None` when it holds none — the same three members [`Self::to_detail`]
    /// reports.
    fn analyzed_recommendation(&self) -> Option<RecommendationSet> {
        recommendation_of(
            &self.strategy,
            &self.target_destination,
            &self.transformation_tool,
        )
    }

    /// Whether this component counts towards the recommendation reported for the server
    /// `server_id`: it runs on that server, and `inclusionStatus` — "indicates whether the
    /// application component has been included for server recommendation or not" — does
    /// not keep it out.
    fn recommended_for_server(&self, server_id: &str) -> bool {
        self.inclusion_status.as_deref() != Some(analysis::EXCLUDE_FROM_ASSESSMENT)
            && self
                .associated_server_ids
                .iter()
                .any(|held| held == server_id)
    }

    /// The strategies a read reports for this component — [`strategy_listings`] as
    /// `ApplicationComponentStrategy`: the option the account prefers first, then the
    /// service's own recommendation when it is a different one.
    pub fn to_component_strategies(&self) -> Vec<ApplicationComponentStrategy> {
        strategy_listings(
            self.analyzed_recommendation(),
            self.preferred_strategy.as_ref(),
        )
        .into_iter()
        .map(|listing| ApplicationComponentStrategy {
            recommendation: Some(listing.recommendation),
            status: listing.status,
            is_preferred: Some(listing.is_preferred),
        })
        .collect()
    }

    /// True when this component satisfies `criteria` = `value`.
    ///
    /// Each `ApplicationComponentCriteria` value names ONE member of the component record
    /// and the value is compared to it for equality — `filterValue` is documented "Specify
    /// the value based on the application component criteria type. For example, if
    /// `applicationComponentCriteria` is set to `SERVER_ID` and `filterValue` is set to
    /// `server1`, then `ListApplicationComponents` returns all the application components
    /// running on `server1`". `SERVER_ID` is the one criteria whose member is a LIST (a
    /// component runs on one or more servers), so a value equal to any of the servers the
    /// component runs on selects it.
    ///
    /// [`CRITERIA_NOT_DEFINED`] names no member, so it selects every component. A value
    /// `ApplicationComponentCriteria` does not declare selects none; a caller never sees
    /// that, because the handler refuses an undeclared criteria before it gets here.
    pub fn matches_criteria(&self, criteria: &str, value: &str) -> bool {
        match criteria {
            CRITERIA_NOT_DEFINED => true,
            "APP_NAME" => self.name.as_deref() == Some(value),
            "APP_TYPE" => self.app_type.as_deref() == Some(value),
            "SERVER_ID" => self
                .associated_server_ids
                .iter()
                .any(|server| server == value),
            "STRATEGY" => self.strategy.as_deref() == Some(value),
            "DESTINATION" => self.target_destination.as_deref() == Some(value),
            "ANALYSIS_STATUS" => self.analysis_status.as_deref() == Some(value),
            "ERROR_CATEGORY" => self.app_unit_error_category.as_deref() == Some(value),
            _ => false,
        }
    }

    /// True when this component came in with one of `groups` — the service's one
    /// [`in_any_group`], over the groups this record holds.
    pub fn in_any_group(&self, groups: &[Group]) -> bool {
        in_any_group(&self.group_id, groups)
    }

    /// Whether an assessment covering the servers `covered` covers this component too.
    ///
    /// A component runs ON a server (`ApplicationComponentDetail$associatedServerId` —
    /// "the ID of the server that the application component is running on"), so an
    /// assessment that collects data for a server collects it for the components running
    /// there, and for no others: a component is in scope exactly when one of the servers it
    /// runs on is. A component no server association was recorded for therefore runs
    /// nowhere an assessment reaches, so no assessment covers it — an assessment covering
    /// no server covers no component at all.
    pub fn runs_on_any(&self, covered: &BTreeSet<String>) -> bool {
        self.associated_server_ids
            .iter()
            .any(|server| covered.contains(server))
    }
}

/// One data collector installed in the account's environment, as this emulator persists
/// it.
///
/// A collector is NOT created through this API: `Collector` is documented "Process data
/// collector that runs in the environment that you specify" and `registeredTimeStamp` is
/// "Time when the collector registered with the service", so a collector enters the store
/// when the appliance installed on-premises registers itself — a path outside the modelled
/// surface, which is why a freshly booted emulator holds none and `ListCollectors` answers
/// the empty list it honestly holds.
///
/// Every member is what that registration recorded, so a read derives nothing from the
/// request that reads it and a member the registration did not carry stays `None` rather
/// than being filled with a value nothing reported. The two time stamps are held as
/// strings because the model types them as `String` rather than as a `timestamp`
/// (`Collector$registeredTimeStamp`, `Collector$lastActivityTimeStamp`): the record keeps
/// exactly the text the collector reported.
#[derive(Debug, Clone, Default)]
pub struct CollectorRecord {
    /// `Collector$collectorId` — a `String` bounded to 1024 with pattern `.*\S.*`. It is
    /// also this collector's key in [`RegionState::collectors`], which is what gives the
    /// list a stable total order to paginate over.
    pub id: String,
    /// `Collector$ipAddress` — "IP address of the server that is hosting the collector."
    pub ip_address: Option<String>,
    /// `Collector$hostName` — "Hostname of the server that is hosting the collector."
    pub host_name: Option<String>,
    /// `Collector$collectorHealth`, one of `CollectorHealth`'s values
    /// ([`COLLECTOR_HEALTH_VALUES`](crate::ops::types::COLLECTOR_HEALTH_VALUES)).
    pub health: Option<String>,
    /// `Collector$collectorVersion` — the version of the collector running in the
    /// environment.
    pub version: Option<String>,
    /// `Collector$registeredTimeStamp` — when the collector registered with the service.
    pub registered_time_stamp: Option<String>,
    /// `Collector$lastActivityTimeStamp` — when the collector last pinged the service.
    pub last_activity_time_stamp: Option<String>,
    /// `Collector$configurationSummary` — the remote sources this collector is configured
    /// to analyse, as the collector reported them. `None` for a collector that reported no
    /// configuration, so the read answers nothing rather than a tree of empty lists.
    pub configuration_summary: Option<ConfigurationSummary>,
}

impl CollectorRecord {
    /// The `Collector` a read reports for this record: its own members, and nothing for
    /// the ones the registration did not carry.
    pub fn to_collector(&self) -> Collector {
        Collector {
            collector_id: Some(self.id.clone()),
            ip_address: self.ip_address.clone(),
            host_name: self.host_name.clone(),
            collector_health: self.health.clone(),
            collector_version: self.version.clone(),
            registered_time_stamp: self.registered_time_stamp.clone(),
            last_activity_time_stamp: self.last_activity_time_stamp.clone(),
            configuration_summary: self.configuration_summary.clone(),
        }
    }
}

/// One page of collectors, as `ListCollectors` answers it.
#[derive(Debug, Default)]
pub struct CollectorPage {
    /// The collectors of this page, in the store's total order.
    pub collectors: Vec<Collector>,
    /// The cursor the next call resumes from, set only when another collector follows
    /// this page.
    pub next_token: Option<String>,
}

/// The account's migration and modernization preferences, as `PutPortfolioPreferences`
/// saved them and `GetPortfolioPreferences` reports them.
///
/// One record per account — the API names no id and takes no input on the read, so there
/// is exactly one of these in the store. `PutPortfolioPreferences` *saves* the
/// preferences, so the record it writes is the whole of the request: a member the request
/// leaves out is not set afterwards, however an earlier call left it. Every member is
/// optional in both directions, so a preference nobody expressed comes back absent rather
/// than as a default nothing chose.
#[derive(Debug, Clone, Default)]
pub struct PortfolioPreferences {
    /// `PutPortfolioPreferencesRequest$prioritizeBusinessGoals`.
    pub prioritize_business_goals: Option<PrioritizeBusinessGoals>,
    /// `PutPortfolioPreferencesRequest$applicationPreferences`.
    pub application_preferences: Option<ApplicationPreferences>,
    /// `PutPortfolioPreferencesRequest$databasePreferences`.
    pub database_preferences: Option<DatabasePreferences>,
    /// `PutPortfolioPreferencesRequest$applicationMode`, one of `ApplicationMode`'s
    /// values ([`APPLICATION_MODE_VALUES`](crate::ops::types::APPLICATION_MODE_VALUES)).
    pub application_mode: Option<String>,
}

impl PortfolioPreferences {
    /// The `GetPortfolioPreferences` answer: this record, member for member.
    ///
    /// The read reports what the save stored and derives nothing, so the two operations
    /// cannot drift apart.
    pub fn to_output(&self) -> GetPortfolioPreferencesOutput {
        GetPortfolioPreferencesOutput {
            prioritize_business_goals: self.prioritize_business_goals.clone(),
            application_preferences: self.application_preferences.clone(),
            database_preferences: self.database_preferences.clone(),
            application_mode: self.application_mode.clone(),
        }
    }
}

/// One Region's portfolio: the resources of this service a request signed for that
/// Region can observe, and the only ones it can.
#[derive(Debug, Default)]
pub struct RegionState {
    /// Every import file task, keyed by its id. A `BTreeMap` is what gives
    /// `ListImportFileTask` a stable total order to paginate over.
    pub import_tasks: BTreeMap<String, ImportFileTask>,
    /// Every assessment, keyed by its id.
    pub assessments: BTreeMap<String, Assessment>,
    /// Every server of this account's portfolio, keyed by its id — the inventory
    /// `ListServers` reports. A `BTreeMap` is what gives that list a stable total order
    /// to sort and paginate over, and it is empty until an import ingests a server.
    pub servers: BTreeMap<String, ServerRecord>,
    /// Every application component of this account's portfolio, keyed by its id — the
    /// processes and databases running on the servers above, which
    /// `GetApplicationComponentDetails` reports one at a time. A `BTreeMap` is what gives
    /// a list over them a stable total order, and it is empty until the account's data is
    /// brought in.
    pub application_components: BTreeMap<String, ApplicationComponentRecord>,
    /// Every recommendation report generation task, keyed by its id: what
    /// `StartRecommendationReportGeneration` mints and `GetRecommendationReportDetails`
    /// reports.
    pub recommendation_reports: BTreeMap<String, RecommendationReport>,
    /// Every data collector installed in this account's environment, keyed by its id —
    /// the collectors `ListCollectors` reports. A `BTreeMap` is what gives that list a
    /// stable total order to paginate over, and it is empty until a collector registers
    /// itself (see [`CollectorRecord`]).
    pub collectors: BTreeMap<String, CollectorRecord>,
    /// The id of the most recently started assessment, which is what
    /// `GetLatestAssessmentId` answers. `None` until one is started.
    pub latest_assessment_id: Option<String>,
    /// This account's migration and modernization preferences in this Region, written by
    /// `PutPortfolioPreferences` and reported by `GetPortfolioPreferences`. `None` until a
    /// save stores them: no preference is set in a fresh account.
    pub portfolio_preferences: Option<PortfolioPreferences>,
    /// The bucket this account named for what this service uploads in this Region, as the
    /// latest start call that named one set it —
    /// `StartAssessmentRequest$s3bucketForReportData` ("The S3 bucket where all the
    /// reports generated by the service are stored") or
    /// `StartImportFileTaskRequest$s3bucketForReportData` ("The S3 bucket where Strategy
    /// Recommendations uploads import results").
    ///
    /// It is where a recommendation report generation task writes its report FILE
    /// ([`report_file`]) and so what the completed task reports as
    /// `RecommendationReportDetails$s3Bucket`. `None` until a start call names one, which
    /// is what makes a report generation in such an account name no file location: there
    /// is nowhere for the file to go ([`NO_REPORT_DATA_BUCKET`]). Held here, beside
    /// [`Self::latest_assessment_id`], because it is the account's standing configuration
    /// rather than any one task's member.
    pub report_data_bucket: Option<String>,
}

/// One completion the clock owes a portfolio, and the record it is owed to.
///
/// What the settlement of a whole portfolio is a list of
/// ([`RegionState::completions_due_by`]): the instant is what orders them, so each
/// completion is materialised over the records the clock had already produced when it fell
/// due.
#[derive(Debug)]
struct OwedCompletion {
    /// The instant this completion fell due at.
    due: SystemTime,
    /// The record it is owed to, by id.
    record: OwedRecord,
}

/// Which record of a portfolio a completion is owed to — the two kinds whose completion
/// changes what another one is materialised over.
#[derive(Debug)]
enum OwedRecord {
    /// An import file task: its completion puts the records it ingested into the inventory.
    Import(String),
    /// An assessment: its completion records the analysis of the resources it covered.
    Assessment(String),
}

impl RegionState {
    /// The import file task `id` names, with any completion the clock owes it already
    /// materialised.
    pub fn settled_import_task_mut(
        &mut self,
        id: &str,
        now: SystemTime,
    ) -> Option<&mut ImportFileTask> {
        self.settle_imports(now);
        self.import_tasks.get_mut(id)
    }

    /// Materialise what the clock owes every import file task of this portfolio: the
    /// terminal status the task reaches, and the records it ingested entering the
    /// inventory the read path reports.
    ///
    /// One task at a time ([`Self::settle_import`]), so the one site that moves an import's
    /// records is the one that gives it its terminal status whichever caller reaches it:
    /// no caller can materialise an import's terminal status without the records that
    /// status means the import brought in. Each record moves once, because a completed task
    /// no longer holds any.
    pub fn settle_imports(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .import_tasks
            .values()
            .filter(|task| task.completion_due.is_some_and(|at| at <= now))
            .map(|task| task.id.clone())
            .collect();
        for id in due {
            self.settle_import(&id, now);
        }
    }

    /// Materialise what the clock owes the ONE import file task `id` names: its terminal
    /// status, and the records it ingested entering the inventory.
    ///
    /// The records were built by the start call, so this only moves them — out of the task
    /// and into the inventory in the same step that completes it. A task the clock owes
    /// nothing yet, and one this portfolio does not hold, are left exactly as they are.
    fn settle_import(&mut self, id: &str, now: SystemTime) {
        let (servers, components) = {
            let Some(task) = self.import_tasks.get_mut(id) else {
                return;
            };
            task.settle(now);
            if !task.is_complete() {
                return;
            }
            (
                ::std::mem::take(&mut task.pending_servers),
                ::std::mem::take(&mut task.pending_components),
            )
        };
        for record in servers {
            self.servers.insert(record.id.clone(), record);
        }
        for record in components {
            self.application_components
                .insert(record.id.clone(), record);
        }
    }

    /// The assessment `id` names, with every completion the clock owes this portfolio
    /// already materialised — on the assessment AND on the resources it covered.
    ///
    /// The WHOLE portfolio is settled ([`Self::settle_all`]) rather than this one record,
    /// because a completion records the analysis of the inventory the clock had already
    /// produced when that completion fell due: the records an import whose completion fell
    /// due first brought in are part of that inventory, and the records of one that fell due
    /// later are not. Materialising a completion is a one-time write
    /// ([`Assessment::settle`] clears the due instant), so a caller that settled a subset
    /// would record an analysis over the wrong inventory once and for all.
    pub fn settled_assessment_mut(&mut self, id: &str, now: SystemTime) -> Option<&mut Assessment> {
        if !self.assessments.contains_key(id) {
            return None;
        }
        self.settle_all(now);
        self.assessments.get_mut(id)
    }

    /// Stop the assessment `id` names, moving the servers it was still collecting for off
    /// their collecting status as well.
    ///
    /// Answers `None` when this portfolio holds no such assessment, and otherwise whether
    /// the stop moved a RUNNING assessment to [`ASSESSMENT_STOPPED`]: an assessment that
    /// already reached a terminal status is left exactly as it was, servers included.
    /// Any completion the clock already owed the assessment is materialised first, so a
    /// stop that arrives after the assessment finished finds it terminal.
    pub fn stop_assessment(&mut self, id: &str, now: SystemTime) -> Option<bool> {
        let assessment = self.settled_assessment_mut(id, now)?;
        let stopped = assessment.stop(now);
        let covered = assessment.covered_servers.clone();
        if stopped {
            self.finish_server_collection(&covered, COLLECTION_STOPPED);
        }
        Some(stopped)
    }

    /// The servers of this portfolio an assessment whose criteria are `targets` covers, by
    /// id.
    ///
    /// `StartAssessmentRequest$assessmentTargets` is "List of criteria for assessment"
    /// over `AssessmentTarget` ("Defines the criteria of assessment"), so the criteria are
    /// what SELECT the servers the assessment collects data for: a server is covered when
    /// every criterion holds for it ([`ServerRecord::matches_target`]), and criteria that
    /// select nothing give an assessment of no server at all. A request naming NO
    /// criterion scopes nothing, so the whole inventory is covered — which is the
    /// environment `StartAssessment` "starts the assessment of".
    ///
    /// The inventory is the one the portfolio holds when the assessment starts, so call
    /// [`Self::settle_all`] first: a server an import whose completion is already due
    /// brought in is part of that inventory.
    pub fn servers_covered_by(&self, targets: &[AssessmentTarget]) -> BTreeSet<String> {
        self.servers
            .values()
            .filter(|server| targets.iter().all(|target| server.matches_target(target)))
            .map(|server| server.id.clone())
            .collect()
    }

    /// Put every server of `covered` into [`COLLECTION_STARTED`]: an assessment just
    /// started covering them, so the collection task is running on each one.
    ///
    /// A server the assessment does not cover is not collecting for it, so it keeps the
    /// status it has. It overwrites whatever status an earlier assessment left on a
    /// covered server, because the status reports the collection that is running NOW; an
    /// assessment covering no server has nothing to move.
    pub fn begin_server_collection(&mut self, covered: &BTreeSet<String>) {
        for server in self.covered_servers_mut(covered) {
            server.data_collection_status = Some(COLLECTION_STARTED.to_string());
        }
    }

    /// Move every server of `covered` whose collection task is still running
    /// ([`COLLECTION_RUNNING`]) to `status`, the per-server face of the terminal status its
    /// assessment reached.
    ///
    /// A server that is not collecting keeps the status it has: a terminal assessment
    /// status says nothing about a collection that was never running, and none about a
    /// server the assessment never covered.
    pub fn finish_server_collection(&mut self, covered: &BTreeSet<String>, status: &str) {
        for server in self.covered_servers_mut(covered) {
            if server
                .data_collection_status
                .as_deref()
                .is_some_and(|held| COLLECTION_RUNNING.contains(&held))
            {
                server.data_collection_status = Some(status.to_string());
            }
        }
    }

    /// The records of the servers `covered` names that this portfolio still holds, to
    /// write through.
    ///
    /// An id the inventory no longer holds names no record, so it is skipped rather than
    /// resurrected; the scope is read against the inventory in one place, so the two
    /// writes an assessment makes over its scope cannot disagree about what it covers.
    fn covered_servers_mut<'a>(
        &'a mut self,
        covered: &'a BTreeSet<String>,
    ) -> impl Iterator<Item = &'a mut ServerRecord> {
        self.servers
            .iter_mut()
            .filter(|(id, _)| covered.contains(id.as_str()))
            .map(|(_, server)| server)
    }

    /// Materialise on the PORTFOLIO what an assessment reaching `COMPLETE` at `at` owes
    /// it: every server of `covered` that was still collecting has finished, and the
    /// analysis that completion stands for is recorded on the resources the assessment
    /// covered.
    ///
    /// One site for both, so no caller can materialise the terminal status of an
    /// assessment without the analysis output that status means the service produced.
    pub fn finish_assessment(&mut self, covered: &BTreeSet<String>, at: SystemTime) {
        self.finish_server_collection(covered, COLLECTION_SUCCESS);
        self.analyze_covered_resources(covered, at);
    }

    /// Record the analysis a completed assessment produced, on the resources it covered:
    /// the servers `covered` names and the application components running on them
    /// ([`ApplicationComponentRecord::runs_on_any`]).
    ///
    /// `StartAssessment` "starts the assessment of an on-premises environment", so an
    /// assessment that reached `COMPLETE` has analysed the part of that environment its
    /// criteria selected: each server and each application component the assessment
    /// covered carries from here on the recommendation the account's saved preferences
    /// select for it ([`analysis::server_recommendation`],
    /// [`analysis::component_recommendation`]) and the instant it was analysed — `at`, the
    /// instant the assessment completed, so the same analysis is reported however late it
    /// is first read.
    ///
    /// A resource the analysis does not cover — one outside the assessment's scope, a
    /// component kept out of the assessment, or one outside the `applicationMode` the
    /// preferences name — is left exactly as it was, rather than being given a
    /// recommendation nothing computed for it.
    pub fn analyze_covered_resources(&mut self, covered: &BTreeSet<String>, at: SystemTime) {
        let preferences = self.portfolio_preferences.clone();
        for server in self.covered_servers_mut(covered) {
            let recommendation = analysis::server_recommendation(preferences.as_ref(), server);
            server.strategy = recommendation.strategy;
            server.target_destination = recommendation.target_destination;
            server.transformation_tool = recommendation.transformation_tool;
            server.last_analyzed = Some(at);
        }
        for component in self.application_components.values_mut() {
            if !component.runs_on_any(covered) {
                continue;
            }
            let Some(recommendation) =
                analysis::component_recommendation(preferences.as_ref(), component)
            else {
                continue;
            };
            component.strategy = recommendation.strategy;
            component.target_destination = recommendation.target_destination;
            component.transformation_tool = recommendation.transformation_tool;
            component.last_analyzed = Some(at);
        }
    }

    /// The recommendation report task `id` names, with any terminal status the clock owes
    /// it already materialised.
    pub fn settled_report_mut(
        &mut self,
        id: &str,
        now: SystemTime,
    ) -> Option<&mut RecommendationReport> {
        let report = self.recommendation_reports.get_mut(id)?;
        report.settle(now);
        Some(report)
    }

    /// The report generation task of this portfolio still in flight at `now`, if there is
    /// one.
    ///
    /// Every stored task gets the terminal status the clock already owed it first, so a
    /// task whose span has passed is not reported as in flight. At most one task is ever
    /// in progress, because a start call made while this answers `Some` coalesces onto
    /// that task — it is answered with the task's own id and mints nothing.
    pub fn ongoing_report_generation(&mut self, now: SystemTime) -> Option<OngoingTask> {
        for report in self.recommendation_reports.values_mut() {
            report.settle(now);
        }
        self.running_report_task()
    }

    /// The same task as [`Self::ongoing_report_generation`], read without materialising
    /// anything: for a caller that has already settled what the clock owes this portfolio.
    fn running_report_task(&self) -> Option<OngoingTask> {
        self.recommendation_reports
            .values()
            .find(|report| report.is_in_progress())
            .map(|report| OngoingTask {
                kind: RECOMMENDATION_REPORT_GENERATION_TASK,
                id: report.id.clone(),
                status: report.status.clone(),
            })
    }

    /// How many servers of this account's portfolio a report scoped by `groups` covers —
    /// the count the task keeps, off the scope itself ([`Self::servers_in_scope`]).
    ///
    /// The inventory is the one the portfolio holds when the report starts, so call
    /// [`Self::settle_all`] first: a server an import whose completion is already due
    /// brought in is part of that inventory, so the count does not depend on whether an
    /// unrelated read materialised the completion first.
    pub fn servers_in_groups(&self, groups: &[Group]) -> i32 {
        count_of(self.servers_in_scope(groups).len())
    }

    /// WHICH servers of this account's portfolio a report scoped by `groups` covers, in the
    /// inventory's own order.
    ///
    /// The ONE place this service decides that scope, so the count a task keeps
    /// ([`Self::servers_in_groups`]) and the resources its report FILE reports on
    /// ([`Self::report_scope`]) cannot select different servers.
    ///
    /// An EMPTY filter names no group, so it scopes nothing and the whole inventory is in
    /// scope; otherwise a server is in scope when it came in with one of the groups named
    /// — both halves of a `Group` agreeing, as [`ServerRecord::in_any_group`] decides it.
    ///
    /// The groups are collected into a set first, so the pass costs one walk over the
    /// inventory and one over the filter rather than the product of the two.
    pub fn servers_in_scope(&self, groups: &[Group]) -> Vec<&ServerRecord> {
        if groups.is_empty() {
            return self.servers.values().collect();
        }
        let wanted: BTreeSet<(Option<&str>, Option<&str>)> = groups
            .iter()
            .map(|group| (group.name.as_deref(), group.value.as_deref()))
            .collect();
        self.servers
            .values()
            .filter(|server| {
                server
                    .group_id
                    .iter()
                    .any(|held| wanted.contains(&(held.name.as_deref(), held.value.as_deref())))
            })
            .collect()
    }

    /// What a report scoped by `groups` reports on: the servers in scope and the
    /// application components running on them, each as the row the report file reports for
    /// it, up to `limit` rows.
    ///
    /// The components are the ones whose stored server associations name a server in scope
    /// — "the IDs of the servers on which the application component is running" — so a
    /// component of a server the filter left out is left out with it, and a component no
    /// server association was recorded for is in no report.
    ///
    /// A scope holding more rows than `limit` is reported as exceeding it and collects
    /// NOTHING ([`report_file::ReportScope::exceeds_limit`]), so the work this does — and
    /// the render that follows it — is bounded by `limit` rather than by the size of the
    /// portfolio the account imported.
    pub fn report_scope(&self, groups: &[Group], limit: usize) -> report_file::ReportScope {
        let servers = self.servers_in_scope(groups);
        let in_scope: BTreeSet<String> = servers.iter().map(|server| server.id.clone()).collect();
        let components: Vec<&ApplicationComponentRecord> = self
            .application_components
            .values()
            .filter(|component| component.runs_on_any(&in_scope))
            .collect();
        if servers.len().saturating_add(components.len()) > limit {
            return report_file::ReportScope {
                exceeds_limit: true,
                ..Default::default()
            };
        }
        report_file::ReportScope {
            servers: servers
                .into_iter()
                .map(report_file::ReportedServer::of)
                .collect(),
            components: components
                .into_iter()
                .map(report_file::ReportedComponent::of)
                .collect(),
            exceeds_limit: false,
        }
    }

    /// The task of this account that has not reached a terminal status at `now`, if there
    /// is one.
    ///
    /// Every stored record gets the terminal status the clock already owed it first
    /// ([`Self::settle_all`]) — owed at this same instant even if nobody had called — so a
    /// task whose span has passed is not reported as ongoing. The three kinds of task this
    /// service runs are looked at in the order they appear in the import-then-assess loop,
    /// and the first one still running is the answer: what a caller has to wait for is that
    /// there is one, not which of them it is.
    pub fn ongoing_task(&mut self, now: SystemTime) -> Option<OngoingTask> {
        self.settle_all(now);
        let importing = self
            .import_tasks
            .values()
            .find(|task| !task.is_complete())
            .map(|task| OngoingTask {
                kind: IMPORT_FILE_TASK,
                id: task.id.clone(),
                status: task.status.clone(),
            });
        let assessing = || {
            self.assessments
                .values()
                .find(|assessment| !assessment.is_terminal())
                .map(|assessment| OngoingTask {
                    kind: ASSESSMENT,
                    id: assessment.id.clone(),
                    status: assessment.status.clone(),
                })
        };
        let reporting = || self.running_report_task();
        importing.or_else(assessing).or_else(reporting)
    }

    /// Materialise every completion the clock owes, for a caller about to read across
    /// all of the records — including what the completion of an assessment owes the
    /// servers it covered ([`Self::finish_assessment`]).
    ///
    /// The completions are materialised in the order they FELL DUE, each one over the
    /// records the clock had already produced at its own due instant: an import whose
    /// completion fell due before an assessment's has brought its records into the
    /// inventory that assessment's completion is analysed over, and one that fell due after
    /// it brings them in afterwards, so no completion records work over a record that had
    /// not arrived when the work was owed.
    ///
    /// Each completion records the analysis of the scope ITS assessment covered, so
    /// several assessments coming due at once produce one analysis each rather than one
    /// over the whole portfolio, and a resource two of their scopes share carries the
    /// analysis of the one that completed last.
    ///
    /// A portfolio with nothing due has nothing to materialise, and that is all it has:
    /// the records an import ingested are emptied out of the task at the same moment its
    /// due instant is cleared ([`Self::settle_import`]), so a record waiting to enter the
    /// inventory is always a record whose task still carries a due instant and is always
    /// reached from here.
    ///
    /// Each owed completion is materialised on the ONE record it is owed to, so the work is
    /// linear in the records this portfolio holds rather than the product of those records
    /// and the instants they fell due at.
    pub fn settle_all(&mut self, now: SystemTime) {
        for owed in self.completions_due_by(now) {
            match owed.record {
                OwedRecord::Import(id) => self.settle_import(&id, now),
                OwedRecord::Assessment(id) => self.settle_assessment(&id, now),
            }
        }
        for report in self.recommendation_reports.values_mut() {
            report.settle(now);
        }
    }

    /// Every completion the clock owes this portfolio at or before `now`, in the order they
    /// fell due.
    ///
    /// Collected before anything is settled, because materialising a completion clears the
    /// due instant it was owed at. The sort is stable and the import file tasks are
    /// collected first, so two completions owed at the very same instant put the import's
    /// records into the inventory before the analysis an assessment's completion records
    /// over them.
    fn completions_due_by(&self, now: SystemTime) -> Vec<OwedCompletion> {
        let mut owed: Vec<OwedCompletion> = self
            .import_tasks
            .values()
            .filter_map(|task| {
                task.completion_due.map(|due| OwedCompletion {
                    due,
                    record: OwedRecord::Import(task.id.clone()),
                })
            })
            .chain(self.assessments.values().filter_map(|assessment| {
                assessment.completion_due.map(|due| OwedCompletion {
                    due,
                    record: OwedRecord::Assessment(assessment.id.clone()),
                })
            }))
            .filter(|owed| owed.due <= now)
            .collect();
        owed.sort_by_key(|owed| owed.due);
        owed
    }

    /// Materialise the completion the clock owes the ONE assessment `id` names, and what
    /// that completion owes the servers it covered ([`Self::finish_assessment`]).
    ///
    /// What the completion owes the portfolio is owed to the servers this assessment
    /// covered, so its own scope is what it is materialised over — several assessments
    /// coming due at once therefore produce one analysis each rather than one over the
    /// whole portfolio, and a resource two of their scopes share carries the analysis of
    /// the one that completed last. An assessment the clock owes nothing yet, and one this
    /// portfolio does not hold, are left exactly as they are.
    fn settle_assessment(&mut self, id: &str, now: SystemTime) {
        let Some(assessment) = self.assessments.get_mut(id) else {
            return;
        };
        let Some(at) = assessment.settle(now) else {
            return;
        };
        let covered = assessment.covered_servers.clone();
        self.finish_assessment(&covered, at);
    }

    /// One page of this store's import file tasks, continuing after the task a
    /// `nextToken` names.
    ///
    /// The order is the store's own total order over the minted ids, which is the order
    /// the tasks were started in: `BTreeMap` iterates its keys sorted and an id's
    /// zero-padded counter suffix sorts the same way it counts. A token is emitted only
    /// when another task actually follows the page, so a caller following tokens never
    /// makes a call that returns nothing.
    ///
    /// Refused with the service's `ValidationException` when `max_results` is outside
    /// the documented 1..=[`MAX_LIST_PAGE_SIZE`], or when the token names no task of this
    /// store. Each task in the page gets the completion the clock already owed it first —
    /// owed at this same instant even if nobody had called.
    pub fn page_of_import_tasks(
        &mut self,
        next_token: Option<&str>,
        max_results: Option<i32>,
        now: SystemTime,
    ) -> Result<ImportTaskPage, ValidationException> {
        let limit = page_size(max_results)?;
        // Every task of this store gets the completion the clock already owed it first —
        // owed at this same instant even if nobody had called — so the page reports the
        // statuses the clock has reached and the records a completed import brought in
        // are in the inventory.
        self.settle_imports(now);

        let after = match next_token {
            Some(token) => {
                let resume_after = decode_page_token(IMPORT_TASK_PAGE_MARKER, token)
                    .ok_or_else(page_token_not_accepted)?;
                if !self.import_tasks.contains_key(&resume_after) {
                    return Err(page_token_not_accepted());
                }
                Bound::Excluded(resume_after)
            }
            None => Bound::Unbounded,
        };

        let mut rest = self.import_tasks.range((after, Bound::Unbounded));
        let mut task_infos = Vec::new();
        let mut last: Option<String> = None;
        for (id, task) in rest.by_ref().take(limit) {
            task_infos.push(task.to_task_information());
            last = Some(id.clone());
        }
        let next_token = match (rest.next(), last) {
            (Some(_), Some(key)) => Some(encode_page_token(IMPORT_TASK_PAGE_MARKER, &key)),
            _ => None,
        };

        Ok(ImportTaskPage {
            task_infos,
            next_token,
        })
    }

    /// One page of this Region's collectors, continuing after the collector a
    /// `nextToken` names.
    ///
    /// The order is the store's own total order over the collector ids, and a token is
    /// emitted only when another collector actually follows the page, so a caller
    /// following tokens never makes a call that returns nothing. Nothing here writes: the
    /// page is taken off the stored records, so two identical calls answer the same thing.
    ///
    /// Refused with the service's `ValidationException` when `max_results` is outside the
    /// documented 1..=[`MAX_LIST_PAGE_SIZE`], or when the token is not one this list
    /// issued for a collector the store still holds.
    pub fn page_of_collectors(
        &self,
        next_token: Option<&str>,
        max_results: Option<i32>,
    ) -> Result<CollectorPage, ValidationException> {
        let limit = page_size(max_results)?;

        let after = match next_token {
            Some(token) => {
                let resume_after = decode_page_token(COLLECTOR_PAGE_MARKER, token)
                    .ok_or_else(page_token_not_accepted)?;
                if !self.collectors.contains_key(&resume_after) {
                    return Err(page_token_not_accepted());
                }
                Bound::Excluded(resume_after)
            }
            None => Bound::Unbounded,
        };

        let mut rest = self.collectors.range((after, Bound::Unbounded));
        let mut collectors = Vec::new();
        let mut last: Option<&String> = None;
        for (id, collector) in rest.by_ref().take(limit) {
            collectors.push(collector.to_collector());
            last = Some(id);
        }
        let next_token = match (rest.next(), last) {
            (Some(_), Some(key)) => Some(encode_page_token(COLLECTOR_PAGE_MARKER, key)),
            _ => None,
        };

        Ok(CollectorPage {
            collectors,
            next_token,
        })
    }

    /// The `AssessmentSummary` a read of this Region's portfolio reports — "the summary of
    /// the assessment results", which is what `GetPortfolioSummary` answers with.
    ///
    /// Every list is COUNTED off the records the portfolio holds ([`counted_by_value`]),
    /// grouped by the member the list's own element type names: the servers by the
    /// operating system they run ([`ServerRecord::summarised_os_type`]) and by the status
    /// of their assessment (`ServerDetail$dataCollectionStatus`, a
    /// `RunTimeAssessmentStatus`), the application components by their `AppType` and by the
    /// status of their analysis (`SrcCodeOrDbAnalysisStatus`), and each of the two kinds by
    /// the strategy the analysis recommended for it ([`strategy_summary`]) — so the summary
    /// and the reads of the resources themselves cannot disagree. `lastAnalyzedTimestamp`,
    /// "the time the assessment was performed", is the latest instant an analysis was
    /// recorded on any of those resources ([`Self::last_analyzed`]).
    ///
    /// A group nothing falls into is not reported, and a list with nothing in it is
    /// reported as absent rather than as an empty list — the same way the records' own
    /// reads report theirs, so an account that holds no portfolio summarises as the empty
    /// portfolio it is instead of as a tree of empty objects. The anti-pattern members are
    /// left unset for the reason [`ServerRecord::to_server_detail`] leaves its own unset:
    /// nothing in this emulator generates an anti-pattern report, so there are no
    /// anti-patterns to count by severity and no report object to name.
    pub fn to_assessment_summary(&self) -> AssessmentSummary {
        let server_strategies =
            strategy_summary(self.servers.values().map(|server| server.strategy.as_deref()));
        let component_strategies = strategy_summary(
            self.application_components
                .values()
                .map(|component| component.strategy.as_deref()),
        );
        let server_os_types: Vec<ServerSummary> =
            counted_by_value(self.servers.values().map(ServerRecord::summarised_os_type))
                .into_iter()
                .map(|(os_type, count)| ServerSummary {
                    server_os_type: Some(os_type),
                    count: Some(count),
                })
                .collect();
        let server_statuses: Vec<ServerStatusSummary> = counted_by_value(
            self.servers
                .values()
                .map(|server| server.data_collection_status.as_deref()),
        )
        .into_iter()
        .map(|(status, count)| ServerStatusSummary {
            run_time_assessment_status: Some(status),
            count: Some(count),
        })
        .collect();
        let app_types: Vec<ApplicationComponentSummary> = counted_by_value(
            self.application_components
                .values()
                .map(|component| component.app_type.as_deref()),
        )
        .into_iter()
        .map(|(app_type, count)| ApplicationComponentSummary {
            app_type: Some(app_type),
            count: Some(count),
        })
        .collect();
        let component_statuses: Vec<ApplicationComponentStatusSummary> = counted_by_value(
            self.application_components
                .values()
                .map(|component| component.analysis_status.as_deref()),
        )
        .into_iter()
        .map(|(status, count)| ApplicationComponentStatusSummary {
            src_code_or_db_analysis_status: Some(status),
            count: Some(count),
        })
        .collect();

        AssessmentSummary {
            list_server_strategy_summary: (!server_strategies.is_empty())
                .then_some(server_strategies),
            list_application_component_strategy_summary: (!component_strategies.is_empty())
                .then_some(component_strategies),
            list_antipattern_severity_summary: None,
            list_application_component_summary: (!app_types.is_empty()).then_some(app_types),
            list_server_summary: (!server_os_types.is_empty()).then_some(server_os_types),
            antipattern_report_s3_object: None,
            antipattern_report_status: None,
            antipattern_report_status_message: None,
            last_analyzed_timestamp: self.last_analyzed().map(::aws_smithy_types::DateTime::from),
            list_application_component_status_summary: (!component_statuses.is_empty())
                .then_some(component_statuses),
            list_server_status_summary: (!server_statuses.is_empty()).then_some(server_statuses),
        }
    }

    /// The latest instant an analysis was recorded on any resource of this portfolio, or
    /// `None` while nothing in it has been analysed.
    ///
    /// `AssessmentSummary$lastAnalyzedTimestamp` is "the time the assessment was
    /// performed", and the instant an assessment's analysis was recorded is what each
    /// resource it covered reports as its own `lastAnalyzedTimestamp`
    /// ([`Self::analyze_covered_resources`]) — so the summary reports the latest of those
    /// rather than a second clock reading, and a portfolio nothing has analysed reports no
    /// time at all.
    fn last_analyzed(&self) -> Option<SystemTime> {
        self.servers
            .values()
            .filter_map(|server| server.last_analyzed)
            .chain(
                self.application_components
                    .values()
                    .filter_map(|component| component.last_analyzed),
            )
            .max()
    }

    /// Whether this account has a data source for an assessment to collect from in this
    /// Region.
    ///
    /// `StartAssessment` "starts the assessment of an on-premises environment", and what
    /// collects the data that assessment is made of is the Strategy Recommendations data
    /// collector installed in that environment ([`CollectorRecord`]) — either registered
    /// with the service, or already holding the discovery data it fetched for a server of
    /// the inventory ([`ServerDiscovery`]). An account with neither has nothing for an
    /// assessment to collect from, so the start is refused ([`no_registered_collector`])
    /// rather than answered with an assessment of nothing.
    ///
    /// Resources a file IMPORT brought in are not such a source: an import declares the
    /// resources to assess, it does not collect from them.
    pub fn has_data_source(&self) -> bool {
        !self.collectors.is_empty()
            || self
                .servers
                .values()
                .any(|server| server.discovery.is_some())
    }
}

/// A task of this Region's portfolio that has not reached a terminal status yet, as
/// [`RegionState::ongoing_task`]
/// reports it.
///
/// Carries what a refusal caused by the task tells the caller: which task of the
/// portfolio is still running, and the non-terminal status it holds.
#[derive(Debug, Clone)]
pub struct OngoingTask {
    /// What kind of task it is, in this service's own words — one of [`IMPORT_FILE_TASK`],
    /// [`ASSESSMENT`] and [`RECOMMENDATION_REPORT_GENERATION_TASK`].
    pub kind: &'static str,
    /// The id the start call minted for it.
    pub id: String,
    /// The non-terminal status it carries.
    pub status: String,
}

/// How this service names an import file task when a refusal reports one as ongoing.
pub const IMPORT_FILE_TASK: &str = "import file task";

/// How it names an assessment.
pub const ASSESSMENT: &str = "assessment";

/// How it names a recommendation report generation task.
pub const RECOMMENDATION_REPORT_GENERATION_TASK: &str = "recommendation report generation task";

/// The `ConflictException` (HTTP 409) this service answers a write of the account-wide
/// inputs its recommendations are computed from while `ongoing` is still running.
///
/// The model documents the error as "Exception to indicate that there is an ongoing task
/// when a new task is created. Return when once the existing tasks are complete", so the
/// message reports exactly that: which task of the account is still running, the status it
/// holds, and that the write succeeds once the task completes. It carries nothing from the
/// request — the condition is one of the STORE, not of what the caller sent.
pub fn ongoing_task_conflict(ongoing: &OngoingTask) -> ConflictException {
    ConflictException {
        message: format!(
            "The {} {} is {}. Retry once the ongoing task is complete.",
            ongoing.kind, ongoing.id, ongoing.status
        ),
    }
}

/// This service's message for a request refused because a value it carries does not
/// satisfy a rule the model declares.
///
/// Strategy Recommendations states that refusal as ONE service-wide sentence — the
/// `ValidationException` the model documents as "The request body isn't valid." — and
/// names neither the member that failed nor the value it rejected. It is a constant of
/// the service, not a rendering of the request, so it is stated once here and used for
/// every operation and every constraint kind (see
/// `EmulatedService::override_refusal`).
pub const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// The `ValidationException` (HTTP 400) this service answers for a request carrying a
/// value outside a rule it enforces but the model does not express — a documented bound
/// the boundary validator cannot refuse.
///
/// Worded with [`INVALID_REQUEST_BODY`], the same one sentence `EmulatedService::
/// override_refusal` gives a modelled constraint violation, so a caller sees one refusal
/// for one logical failure whichever side of the boundary decided it.
pub fn invalid_request_body() -> ValidationException {
    ValidationException {
        message: Some(INVALID_REQUEST_BODY.to_string()),
    }
}

/// This service's message for a request whose parameters it cannot resolve: an id that
/// names no resource of this account.
///
/// Like [`INVALID_REQUEST_BODY`] it is a constant of the service and carries nothing
/// from the request — in particular not the id that failed to resolve, which a caller
/// already holds.
pub const INVALID_OR_MISSING_PARAMETERS: &str =
    "Looks like you provided invalid or missing parameters.";

/// The `ResourceNotFoundException` payload for an import file task id that names no
/// task.
///
/// Takes no id: the message is this service's constant for an unresolvable parameter,
/// so there is nothing from the request to put in it.
pub fn import_task_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(INVALID_OR_MISSING_PARAMETERS.to_string()),
    }
}

/// The refusal of a `nextToken` this service cannot resolve: one it never minted, or one
/// naming a task the store no longer holds.
///
/// A pagination token is an unresolvable parameter, so it earns the same
/// `ValidationException` (HTTP 400) and the same constant sentence
/// ([`INVALID_OR_MISSING_PARAMETERS`]) as any other — the token is NOT echoed back.
pub fn page_token_not_accepted() -> ValidationException {
    ValidationException {
        message: Some(INVALID_OR_MISSING_PARAMETERS.to_string()),
    }
}

/// This service's message for an assessment id that resolves to no assessment, on the
/// operation that reports it as a missing resource.
///
/// `StopAssessment`'s recorded 404 carries this sentence rather than
/// [`INVALID_OR_MISSING_PARAMETERS`]: the service words "I cannot resolve what you sent"
/// and "that resource is not here" differently. Like the other two it is a constant of
/// the service — the recorded response for an unknown id does not interpolate the id, so
/// neither does this.
pub const INVALID_RESOURCE_ID: &str = "Invalid resourceId: ";

/// This service's message for a report it cannot produce because the resource it would
/// name does not exist yet, as `GetLatestAssessmentId` answers it in an account where no
/// assessment has ever been started and `GetPortfolioPreferences` in one where no
/// preferences have ever been saved.
pub const NOTHING_TO_REPORT: &str = "Something went wrong, please try again later.";

/// The prefix every S3 bucket name this service can use has to begin with.
///
/// Documented on each bucket member the service takes — `StartAssessmentRequest`'s
/// `s3bucketForAnalysisData` and `s3bucketForReportData` ("The bucket name must begin
/// with `migrationhub-strategy-`") and `StartImportFileTaskRequest`'s `S3Bucket` and
/// `s3bucketForReportData` ("The bucket name is required to begin with
/// `migrationhub-strategy-`"). The members' own `@pattern`
/// (`[0-9a-z]+[0-9a-z\.\-]*[0-9a-z]+`) and `@length(0,63)` cannot express a prefix, so
/// the boundary validator passes a non-conforming name and the handler enforces it.
pub const BUCKET_NAME_PREFIX: &str = "migrationhub-strategy-";

/// The refusal a bucket member naming a bucket this service may not REACH earns, word for
/// word as Strategy Recommendations words it.
///
/// It is the PERMISSIONS refusal, for a bucket that IS there and that the service may not
/// touch: it names the prefix a bucket it may reach has to carry, which is what the caller
/// has to fix — a permanent condition, and so a different one from
/// [`bucket_not_found`], which says the account holds no such bucket and which a caller
/// fixes by creating it. The sentence names the prefix without its trailing separator, as
/// the service states it, and interpolates nothing from the request: the condition is about
/// the prefix and not about the name that missed it.
pub const BUCKET_PERMISSIONS_REFUSAL: &str = "The service doesn't have permissions to \
     specified S3 Bucket Service doesn't hv permissions to access bucket, please make sure \
     bucket name begins with \"migrationhub-strategy\". [S3PermissionsIsIncorrect]";

/// That refusal as the shape it is answered with.
pub fn bucket_permissions_incorrect() -> ValidationException {
    ValidationException {
        message: Some(BUCKET_PERMISSIONS_REFUSAL.to_string()),
    }
}

/// The refusal a bucket member naming a bucket the account does not hold earns, word for
/// word as Strategy Recommendations words it.
///
/// Its one variable part is the bucket name, taken from the member that failed the check,
/// so a caller that supplied both bucket members is told WHICH name resolved to nothing.
/// It is the condition a caller fixes by creating or correcting the bucket and retrying,
/// which is why it is reported separately from [`BUCKET_PERMISSIONS_REFUSAL`].
pub fn bucket_not_found(bucket: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "S3 bucket \"{bucket}\" does not exist or is not accessible [S3BucketNotFound]"
        )),
    }
}

/// Whether the account holds a bucket named `bucket`, asked of the emulator's own Amazon S3
/// service through the in-process callback.
///
/// A HEAD of the bucket is S3's own "is it there, and can I reach it" question, so the
/// answer is S3's own — read through the same seam an import's file read goes through
/// ([`s3_client`]) rather than from a second copy of S3's inventory kept here, which
/// nothing would write. A HEAD that cannot be answered at all is not a bucket this service
/// can use either, which is what the not-found refusal's own wording states.
pub async fn bucket_is_reachable(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
) -> bool {
    s3_client(ctx, region)
        .head_bucket()
        .bucket(bucket)
        .send()
        .await
        .is_ok()
}

/// Refuse a bucket member naming one this service cannot use, with the refusal of the
/// condition that actually holds.
///
/// `Ok` for an absent member: nothing was named, so there is no bucket to use.
///
/// A named bucket is one this service has to READ the import file out of or WRITE its
/// results into, so the bucket itself is looked up before anything is minted
/// ([`bucket_is_reachable`]) — a name is not a bucket, whatever it spells. The two refusals
/// tell a caller to do opposite things, so each reports the condition that actually holds:
/// a name object storage holds no bucket for is reported as the missing resource it is
/// ([`bucket_not_found`], naming the bucket the request sent), and a bucket that IS there
/// but lies outside [`BUCKET_NAME_PREFIX`] — the namespace this service may be reached
/// under — is reported as the permission failure its name earns
/// ([`BUCKET_PERMISSIONS_REFUSAL`]). Both are `ValidationException` (HTTP 400).
pub async fn check_bucket_usable(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    value: Option<&str>,
) -> Result<(), ValidationException> {
    let Some(bucket) = value else {
        return Ok(());
    };
    if !bucket_is_reachable(ctx, region, bucket).await {
        return Err(bucket_not_found(bucket));
    }
    if !bucket.starts_with(BUCKET_NAME_PREFIX) {
        return Err(bucket_permissions_incorrect());
    }
    Ok(())
}

/// The bounds `StartImportFileTaskRequest$name` declares: `@length(min: 1, max: 50)`
/// (`smithy_query(query="operation", operation="StartImportFileTask")`).
pub const IMPORT_NAME_LENGTH: ::std::ops::RangeInclusive<usize> = 1..=50;

/// Whether `character` is one `StartImportFileTaskRequest$name`'s `@pattern`
/// (`[a-zA-Z0-9_-]+`) admits: an ASCII letter or digit, an underscore or a hyphen — so no
/// space and no other punctuation.
fn import_name_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_' || character == '-'
}

/// Refuse an import name the model does not admit.
///
/// `StartImportFileTaskRequest$name` carries its `@length(1,50)` and its `@pattern`
/// ([`import_name_character`]) ON THE MEMBER, over an unconstrained `String` target, and
/// the generated boundary validator enforces only what a member's TARGET shape declares
/// (and no `@pattern` at all) — so this constraint is the handler's to decide. Refused
/// with the service's one modelled-constraint sentence ([`invalid_request_body`]), which
/// is what `EmulatedService::override_refusal` gives every constraint the validator does
/// refuse, so one logical failure reads the same whichever side decided it.
pub fn check_import_name(name: &str) -> Result<(), ValidationException> {
    let counted = name.chars().count();
    if !IMPORT_NAME_LENGTH.contains(&counted) || !name.chars().all(import_name_character) {
        return Err(invalid_request_body());
    }
    Ok(())
}

/// The longest `s3key` the model admits: `@length(max: 1024)` on
/// `StartImportFileTaskRequest$s3key`.
pub const MAX_S3_KEY_CHARACTERS: usize = 1024;

/// Refuse an S3 key the model does not admit.
///
/// `s3key`'s `@pattern` is `.*\S.*` — the value has to carry a non-whitespace character,
/// so a blank key names no object — within its `@length(0,1024)`. Both sit on the member
/// over an unconstrained `String`, so neither reaches the boundary validator.
pub fn check_s3_key(key: &str) -> Result<(), ValidationException> {
    if key.chars().count() > MAX_S3_KEY_CHARACTERS
        || !key.chars().any(|character| !character.is_whitespace())
    {
        return Err(invalid_request_body());
    }
    Ok(())
}

/// The longest bucket name the model admits: `@length(max: 63)` on `importS3Bucket` and on
/// the other bucket members, which is also S3's own bound on a bucket name.
pub const MAX_BUCKET_NAME_CHARACTERS: usize = 63;

/// Refuse a bucket name the model does not admit.
///
/// Every bucket member of this service carries the `@pattern`
/// `[0-9a-z]+[0-9a-z\.\-]*[0-9a-z]+`: a name of at least two characters made of LOWER-CASE
/// letters, digits, dots and hyphens that both starts and ends with a letter or a digit.
/// The pattern is the half of those members' constraints the boundary validator does not
/// enforce (it does refuse a name past [`MAX_BUCKET_NAME_CHARACTERS`], which the target
/// shape declares, and that bound is restated here so one function decides the whole
/// modelled rule).
///
/// `Ok` for an absent member: an optional bucket that was not named carries no value to
/// hold to the pattern. This is the MODELLED rule alone — whether the service may use the
/// bucket it names is [`check_bucket_usable`], a different refusal, so this one runs first.
pub fn check_bucket_name(value: Option<&str>) -> Result<(), ValidationException> {
    let Some(bucket) = value else {
        return Ok(());
    };
    let edge = |character: char| character.is_ascii_lowercase() || character.is_ascii_digit();
    let inner = |character: char| edge(character) || character == '.' || character == '-';
    let counted = bucket.chars().count();
    if !(2..=MAX_BUCKET_NAME_CHARACTERS).contains(&counted)
        || !bucket.chars().all(inner)
        || !bucket.chars().next().is_some_and(edge)
        || !bucket.chars().next_back().is_some_and(edge)
    {
        return Err(invalid_request_body());
    }
    Ok(())
}

/// `StartAssessment`'s refusal in an account that has registered no data collector, word
/// for word as Strategy Recommendations words it.
///
/// An assessment is made of the data a collector collects from the environment, so an
/// account that registered none has no data source for one
/// ([`RegionState::has_data_source`]): the service refuses the start with
/// `ValidationException` (HTTP 400) and tells the caller to register a collector first,
/// rather than minting an assessment of an environment nothing can collect from. It is the
/// answer whatever `assessmentDataSourceType` the request names — recorded wire behaviour
/// of `POST /start-assessment` shows the same refusal for the default and for
/// `ManualImport`, because the type says where the resources to assess came from and not
/// what collects their data. The operation's own error list omits the shape, so it is
/// widened into its error channel with `extra_errors`.
pub const NO_REGISTERED_COLLECTOR: &str =
    "No connectors are registered. please register collector to perform assessment";

/// That refusal as the shape it is answered with. It carries nothing from the request:
/// the condition is one of the ACCOUNT, not of what the caller sent.
pub fn no_registered_collector() -> ValidationException {
    ValidationException {
        message: Some(NO_REGISTERED_COLLECTOR.to_string()),
    }
}

/// Refuse an operation that was given no bucket for a member it cannot run without.
///
/// For a member the model leaves optional but the service needs: the refusal names the
/// MEMBER and the prefix a bucket for it has to carry, which is what the caller has to
/// fix. `action` is what the operation does with the bucket, so the message reads for the
/// operation it came from. A member the model marks `@required` never reaches here — the
/// boundary validator refuses its absence first.
pub fn require_bucket(
    member: &str,
    action: &str,
    value: Option<&str>,
) -> Result<(), ValidationException> {
    if value.is_some() {
        return Ok(());
    }
    Err(ValidationException {
        message: Some(format!(
            "S3 bucket {member} is required for {action}. Please specify s3 bucket \
             with \"{BUCKET_NAME_PREFIX}\" prefix"
        )),
    })
}

/// Refuse a request member whose value is not one the enumeration it targets declares.
///
/// `declared` is the generated value list of the member's target shape (the `*_VALUES`
/// constants in [`crate::ops::types`], rendered from the model's own `enum` definitions),
/// so the catalog a value is held to is the model's and never one written out by hand.
///
/// An ABSENT member is `Ok`: an optional member that was not sent carries no value that
/// could be outside the enumeration, and a `@required` one never reaches here because the
/// boundary validator refuses its absence first. The handler has to do this because the
/// boundary validator admits ANY string for an enum-targeting member — an `enum`'s value
/// list is not a constraint it enforces — so without this check an undeclared value would
/// be persisted and reported back as if the service had accepted it.
///
/// Refused with the service's one `ValidationException` (HTTP 400) sentence
/// ([`invalid_request_body`]), the same refusal every other rule the model cannot express
/// answers; each operation maps it into its own error channel.
pub fn check_enum(value: Option<&str>, declared: &[&str]) -> Result<(), ValidationException> {
    match value {
        Some(named) if !declared.contains(&named) => Err(invalid_request_body()),
        _ => Ok(()),
    }
}

/// The strategy option a config update stores against the resource it names, or `None`
/// when the request names none to store.
///
/// One rule for every `strategyOption` this service accepts, whichever resource the
/// preference is held against: each of the shape's three enum-targeting members
/// (`strategy`, `targetDestination`, `toolName`) is held to its own catalog by
/// [`check_enum`], and an option whose every one of those three is absent selects no
/// strategy, no destination and no tool, so there is nothing to store — a config update
/// writes the members the request names, and that request names none. `isPreferred`
/// carries no selection of its own, so an option bearing only it is that case.
///
/// Refused with the service's one `ValidationException`; each operation lifts it into its
/// own error channel.
pub fn preferred_strategy_option(
    option: Option<StrategyOption>,
) -> Result<Option<StrategyOption>, ValidationException> {
    let Some(option) = option else {
        return Ok(None);
    };
    check_enum(option.strategy.as_deref(), STRATEGY_VALUES)?;
    check_enum(option.target_destination.as_deref(), TARGET_DESTINATION_VALUES)?;
    check_enum(option.tool_name.as_deref(), TRANSFORMATION_TOOL_NAME_VALUES)?;
    let names_a_strategy = option.strategy.is_some()
        || option.target_destination.is_some()
        || option.tool_name.is_some();
    Ok(names_a_strategy.then_some(option))
}

/// Hold every entry of an import-group list to the `GroupName` catalog.
///
/// A `Group`'s `name` targets `GroupName`, so the same enum rule as [`check_enum`] applies
/// to it one level down, inside each element of the list: a request naming a group kind
/// the model does not declare is refused rather than stored. Its `value` is a free-form
/// string and is held only to what the model expresses about it.
pub fn check_group_names(groups: &[Group]) -> Result<(), ValidationException> {
    groups
        .iter()
        .try_for_each(|group| check_enum(group.name.as_deref(), GROUP_NAME_VALUES))
}

/// Whether a list is reported in descending order, from the `sort` the request carries.
///
/// Every `sort` member of this service targets `SortOrder` and is documented the same way
/// — "Specifies whether to sort by ascending (ASC) or descending (DESC) order" — and names
/// no key, so the direction is all a request chooses and the rule is one for every list
/// here. An ABSENT `sort` is ascending ([`SORT_ASCENDING`]); a value `SortOrder` does not
/// declare is refused with the service's [`invalid_request_body`], since the boundary
/// validator admits any string for an enum-targeting member.
pub fn sort_descending(sort: Option<&str>) -> Result<bool, ValidationException> {
    match sort {
        None | Some(SORT_ASCENDING) => Ok(false),
        Some(SORT_DESCENDING) => Ok(true),
        Some(_) => Err(invalid_request_body()),
    }
}

/// `GetAssessment`'s refusal of an `id` that names no assessment of this account.
///
/// Strategy Recommendations answers that read with its invalid-parameter refusal —
/// `ValidationException`, HTTP 400 — rather than a not-found, which is what the recorded
/// wire behaviour of `GET /get-assessment/{id}` shows for a pattern- and length-legal id
/// the account was never issued. The status and the error code are the requirement; the
/// message is this service's one sentence for an unresolvable parameter
/// ([`INVALID_OR_MISSING_PARAMETERS`]), which carries nothing from the request.
pub fn assessment_id_not_accepted() -> ValidationException {
    ValidationException {
        message: Some(INVALID_OR_MISSING_PARAMETERS.to_string()),
    }
}

/// `StopAssessment`'s refusal of an `assessmentId` that names no assessment: the
/// service's `ResourceNotFoundException` (HTTP 404, "The specified ID in the request is
/// not found."), worded with [`INVALID_RESOURCE_ID`].
pub fn assessment_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(INVALID_RESOURCE_ID.to_string()),
    }
}

/// `GetRecommendationReportDetails`'s refusal of an `id` that names no report generation
/// task of this account: the not-found its own error list carries
/// (`ResourceNotFoundException`, HTTP 404, "The specified ID in the request is not
/// found."), worded with the service's [`INVALID_RESOURCE_ID`] sentence.
///
/// Like every other refusal of this service it interpolates nothing from the request —
/// the caller already holds the id it sent.
pub fn recommendation_report_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(INVALID_RESOURCE_ID.to_string()),
    }
}

/// The refusal of a `serverId` that names no server of this account's portfolio, which is
/// what `UpdateServerConfig` and `GetServerStrategies` answer for one: the not-found both
/// operations' error lists carry (`ResourceNotFoundException`, HTTP 404, "The specified ID
/// in the request is not found."), worded with the service's [`INVALID_RESOURCE_ID`]
/// sentence.
///
/// Takes no id: like every other refusal of this service it interpolates nothing from the
/// request — the caller already holds the id it sent.
pub fn server_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(INVALID_RESOURCE_ID.to_string()),
    }
}

/// The refusal of an `applicationComponentId` that names no application component of
/// this account's portfolio: the not-found this operation's error list carries
/// (`ResourceNotFoundException`, HTTP 404, "The specified ID in the request is not
/// found."), worded with the service's [`INVALID_RESOURCE_ID`] sentence — the same
/// refusal its sibling reads answer for an id of their own that resolves to nothing.
///
/// Takes no id: like every other refusal of this service it interpolates nothing from the
/// request — the caller already holds the id it sent.
pub fn application_component_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(INVALID_RESOURCE_ID.to_string()),
    }
}

/// `GetLatestAssessmentId`'s answer in an account where no assessment has ever been
/// started: `ResourceNotFoundException`, HTTP 404.
///
/// The operation takes no input at all, so there is no value a caller could have sent
/// wrongly and the condition is not a refusal of the request: what is missing is the
/// resource the report would name. The service answers that with its not-found shape
/// (HTTP 404, "The specified ID in the request is not found."), which is what the
/// recorded wire behaviour of `GET /get-latest-assessment-id` shows for an account that
/// has started nothing — exactly as the sibling input-less read answers an account with
/// no preferences record (see [`no_portfolio_preferences`]). The operation's own error
/// list omits the shape, so it is widened into its error channel with `extra_errors`; a
/// narrow list is not a licence to substitute a different shape.
///
/// The status and the error code are the requirement; the message is this service's
/// sentence for a report it cannot produce because the resource it would name does not
/// exist yet ([`NOTHING_TO_REPORT`]), and the operation has no input to echo into it.
pub fn no_latest_assessment() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(NOTHING_TO_REPORT.to_string()),
    }
}

/// `GetPortfolioPreferences`'s answer in an account where no preferences have ever been
/// saved: there is no preferences record to report, so the absent resource is reported
/// with `ResourceNotFoundException` (HTTP 404), the not-found this operation's own error
/// list carries — never a 200 whose every member is absent, which would claim the account
/// holds a record nothing wrote.
///
/// The operation takes no input, so the message carries nothing from the request: it is
/// this service's sentence for a report it cannot produce because the resource it would
/// name does not exist yet ([`NOTHING_TO_REPORT`]), the same one `GetLatestAssessmentId`
/// answers in an account with no assessment.
pub fn no_portfolio_preferences() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(NOTHING_TO_REPORT.to_string()),
    }
}

/// The key prefix every import status report object is written under, inside the bucket
/// `StartImportFileTaskRequest$s3bucketForReportData` named.
///
/// Neither AWS nor the model publishes the key layout, so the layout is the emulator's
/// choice; what it has to be is DERIVED from the task it reports on (see
/// [`import_status_report_key`]), so the `statusReportS3Key` a read reports names the one
/// object that import wrote and two imports never write over each other.
pub const IMPORT_STATUS_REPORT_PREFIX: &str = "import-status-reports";

/// The key of the status report object the import file task `task` writes.
///
/// Built from the task's minted id alone, so it is stable for the life of the task and
/// well formed for every import name the boundary validator admits.
pub fn import_status_report_key(task: &ImportFileTask) -> String {
    format!(
        "{IMPORT_STATUS_REPORT_PREFIX}/{}/status-report.json",
        task.id
    )
}

/// A stored instant as the `date-time` string a report document carries, or `None` when
/// the host clock is outside the representable range.
fn rendered_timestamp(at: SystemTime) -> Option<String> {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// The status report document the import file task `task` writes to its report bucket,
/// reporting the outcome `ingestion` of every record its file declared.
///
/// `GetImportFileTaskResponse$statusReportS3Key` documents what the object holds — "The
/// report contains details about whether each record imported successfully or why it did
/// not" — so every value here comes from the import itself: the task it belongs to, where
/// its input file was read from, the two record counts it reports, and one entry per
/// record outcome, carrying the id the record was imported under or the reason it was not.
/// For an import whose file could not be read at all there is no record to list, so the
/// document reports why instead. The serialization (JSON) is the emulator's choice; the
/// content is computed from state and never canned.
pub fn import_status_report_document(
    task: &ImportFileTask,
    ingestion: &import_file::Ingestion,
) -> String {
    let records: Vec<::serde_json::Value> = ingestion
        .outcomes
        .iter()
        .map(|outcome| {
            ::serde_json::json!({
                "resourceType": outcome.resource_type,
                "resourceName": outcome.resource_name,
                "resourceId": outcome.resource_id,
                "imported": outcome.imported(),
                "failureReason": outcome.reason,
            })
        })
        .collect();
    ::serde_json::json!({
        "importTaskId": task.id,
        "importName": task.import_name,
        "importFileS3Bucket": task.input_s3_bucket,
        "importFileS3Key": task.input_s3_key,
        "startTime": rendered_timestamp(task.start_time),
        "status": task.terminal_status,
        "numberOfRecordsSuccess": task.number_of_records_success,
        "numberOfRecordsFailed": task.number_of_records_failed,
        // Why the import read no record at all, for an import that could not read its
        // file; absent for one that did read it.
        "failureReason": ingestion.unreadable,
        // One entry per record the import read from its file, each carrying that record's
        // own outcome — which is what the two counts above count.
        "records": records,
    })
    .to_string()
}

/// Read the import file at (`bucket`, `key`) through the in-process callback, as the text
/// of the document it holds.
///
/// The object is one a caller wrote, so its size is caller-chosen: the body is read a
/// chunk at a time and the read is abandoned the moment it passes
/// [`import_file::MAX_IMPORT_FILE_BYTES`], so nothing larger than that ceiling is ever
/// buffered, whatever length the response advertises. The `Err` carries the reason the
/// object could not be read — no bucket of that name, no object under that key, an object
/// past the ceiling — for the caller to report rather than to count records nothing read.
pub async fn fetch_import_file(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
    key: &str,
) -> Result<String, String> {
    let object = s3_client(ctx, region)
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .map_err(|error| {
            ::aws_smithy_types::error::display::DisplayErrorContext(&error).to_string()
        })?;
    let too_large = || {
        format!(
            "the import file is larger than the {} bytes an import reads",
            import_file::MAX_IMPORT_FILE_BYTES
        )
    };
    let mut body = object.body;
    let mut read: Vec<u8> = Vec::new();
    while let Some(chunk) = body.try_next().await.map_err(|error| error.to_string())? {
        if read.len().saturating_add(chunk.len()) > import_file::MAX_IMPORT_FILE_BYTES {
            return Err(too_large());
        }
        read.extend_from_slice(&chunk);
    }
    String::from_utf8(read).map_err(|error| error.to_string())
}

/// An Amazon S3 client over the in-process callback into this same emulator.
///
/// The callback is the framework's seam for calling another emulated service: it routes
/// in process, so no network client is configured here and this crate names no other
/// service's crate. `region` is the request's own signing region, so the import writes
/// its report where the caller's own S3 requests go.
pub fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a virtual-host
        // bucket name is not a host this emulator resolves, so the bucket is addressed
        // in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or watch for
        // a stalled stream, and each of those would need a sleep implementation this
        // client is not given. The same goes for caching the identity: the credentials
        // are handed over directly.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// Write `body` as the object `key` of `bucket`, through the in-process callback.
///
/// `Ok` only once the object is THERE, which is what lets a task record the key: a later
/// read names a location only for an object a caller can fetch. The `Err` carries the
/// reason the object could not be written — no bucket of that name, say — for the caller to
/// report rather than to name a file nothing wrote.
///
/// One write for every object this service produces (an import's status report, a
/// recommendation report file), so `content_type` is what tells a fetching caller which of
/// them it holds.
pub async fn put_object(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
    key: &str,
    body: Vec<u8>,
    content_type: &str,
) -> Result<(), String> {
    s3_client(ctx, region)
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(::aws_sdk_s3::primitives::ByteStream::from(body))
        .content_type(content_type)
        .send()
        .await
        .map(|_| ())
        .map_err(|error| {
            ::aws_smithy_types::error::display::DisplayErrorContext(&error).to_string()
        })
}

/// Write the import's status report document `body` as the object `key` of `bucket`.
pub async fn put_import_status_report(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
    key: &str,
    body: String,
) -> Result<(), String> {
    put_object(
        ctx,
        region,
        bucket,
        key,
        body.into_bytes(),
        "application/json",
    )
    .await
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>)
}
