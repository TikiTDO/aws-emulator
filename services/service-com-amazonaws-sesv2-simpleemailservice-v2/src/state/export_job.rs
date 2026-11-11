// SPDX-License-Identifier: CC0-1.0

//! The export-job plane's records: one export job, the status its run reports, how a
//! cancellation ends that run, and the wording every operation that addresses a job by
//! its id refuses with.

use super::*;

/// The status of an export job's run, from the model's `JobStatus` enum.
///
/// The enum also carries `FAILED` ("an error occurred during processing"), which nothing
/// here reaches: running an emulated export is bookkeeping and cannot fail. `CANCELLED`
/// is reached only by a request that cancels the job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportJobStatus {
    /// `CREATED`: "job has just been created".
    Created,
    /// `PROCESSING`: "job is processing".
    Processing,
    /// `COMPLETED`: "job has completed processing successfully".
    Completed,
    /// `CANCELLED`: the run was stopped by a request to cancel the job, so it never
    /// completes and never writes a file.
    Cancelled,
}

impl ExportJobStatus {
    /// Whether this status is one a run never leaves again.
    ///
    /// A job whose run has already ended — whether it completed or was cancelled — has
    /// no transition left for the clock to owe it, and nothing can cancel it either.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }

    /// The modelled `JobStatus` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "CREATED",
            Self::Processing => "PROCESSING",
            Self::Completed => "COMPLETED",
            Self::Cancelled => "CANCELLED",
        }
    }
}

/// `ExportSourceType`'s value for "the metrics export", whose data source is a
/// `MetricsDataSource` (see `crate::ops::types::EXPORT_SOURCE_TYPE_VALUES`).
pub const EXPORT_SOURCE_METRICS_DATA: &str = "METRICS_DATA";

/// `ExportSourceType`'s value for "the Message Insights export".
pub const EXPORT_SOURCE_MESSAGE_INSIGHTS: &str = "MESSAGE_INSIGHTS";

/// The `ExportSourceType` the data source `data_source` selects, or `None` when it
/// selects neither or both.
///
/// `ExportDataSource` "can only contain one of `MetricsDataSource` or
/// `MessageInsightsDataSource` object", and which one it carries is what the job's
/// source type is: a request that names neither, or both, selects no source at all.
pub fn export_source_type(
    data_source: &crate::ops::types::ExportDataSource,
) -> Option<&'static str> {
    match (
        data_source.metrics_data_source.as_ref(),
        data_source.message_insights_data_source.as_ref(),
    ) {
        (Some(_), None) => Some(EXPORT_SOURCE_METRICS_DATA),
        (None, Some(_)) => Some(EXPORT_SOURCE_MESSAGE_INSIGHTS),
        _ => None,
    }
}

/// How this service words `BadRequestException` for an `ExportDataSource` that
/// selects no one data source.
///
/// The sentence states the modelled rule — the structure carries exactly one of the
/// two — and carries no part of the request: what is wrong is which members are
/// present, which the sentence already names.
pub fn invalid_export_data_source() -> String {
    "ExportDataSource must contain exactly one of MetricsDataSource or \
     MessageInsightsDataSource."
        .to_string()
}

/// How this service words `BadRequestException` for a create request that carries
/// `ExportDestination$S3Url`.
///
/// "SES sets this value. It's returned only in the `GetExportJob` response... If you
/// include this field in a `CreateExportJob` request, the request fails with a
/// `BadRequestException`" — so the sentence names the member and says who sets it,
/// and carries no part of the request.
pub fn export_destination_s3_url_is_set_by_ses() -> String {
    "ExportDestination.S3Url is set by SES and must not be included in a \
     CreateExportJob request."
        .to_string()
}

/// How this service words `NotFoundException` for a `JobId` the account holds no export
/// job for, stated once for every operation that reads an export job by its id.
///
/// `NotFoundException` is documented only as "the resource you attempted to access
/// doesn't exist", which names no resource, so the sentence names the resource kind and
/// the requested id, which is its only variable part, taken from the request — the same
/// wording this service gives every other resource it is asked for by id
/// ([`import_job_not_found`]).
pub fn export_job_not_found(job_id: &str) -> String {
    format!("Export job {job_id} does not exist.")
}

/// How this service words `BadRequestException` for a request to cancel an export job
/// whose run has already ended.
///
/// A cancellation stops a run that is still going; a run that has already reached a
/// status it never leaves ([`ExportJobStatus::is_terminal`]) has nothing left to stop.
/// The model documents no sentence for `BadRequestException`, so the fixed text is the
/// one a recording of this surface answers this refusal with — the angle brackets around
/// the status included — and its one variable part is the `JobStatus` the job is
/// actually in, read from the stored job rather than from any one run.
pub fn export_job_not_cancellable(status: ExportJobStatus) -> String {
    format!(
        "Cannot cancel an export job that has a <{}> status.",
        status.as_str()
    )
}

/// One export job: the data the export draws on, the format it is written in, how far
/// its run has got, and when the job was created and completed.
#[derive(Debug, Clone)]
pub struct ExportJobRecord {
    /// The `JobId` this record is keyed by, minted when the job was created and
    /// stable for the life of the job.
    pub job_id: String,
    /// The `ExportSourceType` the job's data source is of, which is the source type
    /// every read of the job reports (see `crate::ops::types::EXPORT_SOURCE_TYPE_VALUES`).
    pub source_type: String,
    /// The `ExportDataSource` the job was created for, exactly as the create
    /// supplied it — the dimensions, metrics, filters and date range the export
    /// draws on, which `GetExportJob` reports back as "the data source of the export
    /// job".
    pub data_source: crate::ops::types::ExportDataSource,
    /// `ExportDestination$DataFormat`: "the data format of the final export job
    /// file", as the create supplied it. The destination's `S3Url` is not stored,
    /// because SES sets that value rather than the caller.
    pub data_format: String,
    /// How far the run has got.
    pub status: ExportJobStatus,
    /// When the job was created.
    pub created_at: SystemTime,
    /// When the `CREATED` -> `PROCESSING` transition falls due.
    pub processing_due_at: SystemTime,
    /// When the `PROCESSING` -> `COMPLETED` transition falls due.
    pub completion_due_at: SystemTime,
    /// When the run completed, once it has.
    pub completed_at: Option<SystemTime>,
    /// The file the run wrote, once it has completed: "SES writes the export file to
    /// a location that it manages", and this is that location. Written ONCE, as the
    /// run falls due ([`ExportFile`]), so every download of the job serves the same
    /// bytes and the statistics reported for the job count exactly those records.
    pub file: Option<ExportFile>,
}

impl ExportJobRecord {
    /// A job as a create stores it: just created, with both transitions of its run
    /// already due at known instants.
    ///
    /// The data source and the data format are the create's to fill in, the way an
    /// endpoint's tags are: a reader of the job reports whatever the request that
    /// created it supplied.
    pub fn new(job_id: String, source_type: &str, now: SystemTime) -> Self {
        let processing_due_at = now.checked_add(EXPORT_JOB_PROCESSING_SETTLE).unwrap_or(now);
        Self {
            job_id,
            source_type: source_type.to_string(),
            data_source: crate::ops::types::ExportDataSource::default(),
            data_format: String::new(),
            status: ExportJobStatus::Created,
            created_at: now,
            processing_due_at,
            completion_due_at: processing_due_at
                .checked_add(EXPORT_JOB_COMPLETION_SETTLE)
                .unwrap_or(processing_due_at),
            completed_at: None,
            file: None,
        }
    }

    /// Apply whichever transitions of the run are due at `now`.
    ///
    /// Both are owed by the clock, so applying them changes nothing a later caller
    /// would otherwise have seen — and a run whose whole span has already elapsed
    /// settles straight to `COMPLETED` rather than one status per call.
    ///
    /// The file the run writes is the store's to render ([`ExportFile`]), because the
    /// records it draws on are the account's rather than the job's: the store applies
    /// [`Self::completion_falls_due`] first and hands the file in before calling this.
    pub fn settle(&mut self, now: SystemTime) {
        // A run that has already ended is owed no transition by the clock: neither a
        // completed run nor a cancelled one ever moves again.
        if self.status.is_terminal() {
            return;
        }
        if self.status == ExportJobStatus::Created && now >= self.processing_due_at {
            self.status = ExportJobStatus::Processing;
        }
        // A run is complete only once the file it is complete BY has been written, so a
        // read never reports a finished run with nothing to download. The write is the
        // store's ([`ExportFile`]), and what it has not got to yet stays owed.
        if self.status == ExportJobStatus::Processing
            && now >= self.completion_due_at
            && self.file.is_some()
        {
            self.status = ExportJobStatus::Completed;
            // The run finished when it fell due, not when somebody first looked.
            self.completed_at = Some(self.completion_due_at);
        }
    }

    /// Whether the run's completion falls due at `now` and has not been applied yet,
    /// which is the one instant its file is written at.
    pub fn completion_falls_due(&self, now: SystemTime) -> bool {
        !self.status.is_terminal() && now >= self.completion_due_at
    }

    /// Stop this job's run, as a request to cancel the job does: the run reaches
    /// `CANCELLED` and never completes, so it writes no export file.
    ///
    /// A job whose run has already ended ([`ExportJobStatus::is_terminal`]) is left
    /// exactly as it stood: there is nothing left to stop. `CancelExportJob` refuses
    /// such a request before it gets here, so that guard is the same rule stated once
    /// for every other caller rather than a second, differing one.
    pub fn cancel(&mut self) {
        if self.status.is_terminal() {
            return;
        }
        self.status = ExportJobStatus::Cancelled;
        // "The timestamp of when the export job was completed": a run that was stopped
        // never completed, so there is no such instant to report, and the file a
        // completing run would have written is never written.
        self.completed_at = None;
        self.file = None;
    }

    /// This job's position in a listing of the account's export jobs: the instant it
    /// was created, then its id.
    ///
    /// The instant leads, so the listing is in creation order; the id breaks the tie,
    /// so the order is total even for two jobs created in the same millisecond. The
    /// key is a position rather than a record, which is what keeps a page token valid
    /// when the job it names is no longer there.
    pub fn listing_key(&self) -> String {
        let millis = self
            .created_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        // Fixed-width hexadecimal, so comparing two keys as text orders them the way
        // comparing the instants they carry would.
        format!("{millis:032x}:{}", self.job_id)
    }

    /// The modelled `ExportJobSummary` view of this record, as `ListExportJobs`
    /// reports it.
    pub fn summary(&self) -> crate::ops::types::ExportJobSummary {
        crate::ops::types::ExportJobSummary {
            job_id: Some(self.job_id.clone()),
            export_source_type: Some(self.source_type.clone()),
            job_status: Some(self.status.as_str().to_string()),
            created_timestamp: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            // "The timestamp of when the export job was completed": a run that has
            // not completed has no such instant, so the member is absent until it
            // does.
            completed_timestamp: self.completed_at.map(::aws_smithy_types::DateTime::from),
        }
    }
}
