// SPDX-License-Identifier: CC0-1.0

//! The IoT JOBS store, shared by every service that creates an IoT job and by the
//! plane that reports one.
//!
//! An AWS IoT job is minted by more than one emulated control plane: the IoT control
//! plane creates one directly (`iot`: `CreateJob`) and one per OTA update it deploys
//! (`iot`: `CreateOTAUpdate`), and IoT Greengrass V2 creates one for every deployment
//! it applies to a target (`greengrassv2`: `CreateDeployment`, whose response reports
//! that job's `iotJobId` and `iotJobArn`). Whoever mints it, the job is read, deleted
//! and waited on through the IoT jobs plane (`iot`: `DescribeJob`, `GetJobDocument`,
//! `DeleteJob`, `DescribeJobExecution`), so a job one service creates is the job the
//! other has to find: all of them reach ONE store — this type, registered once in the
//! shared engine — instead of each keeping its own copy keyed by its own state struct.
//!
//! Nothing here names a wire shape or a Smithy type: a service crate projects an
//! [`IotJob`] onto the shapes ITS model declares (`Job`, `JobSummary`, …). What lives
//! here is the record, the lookups a reader needs and the quota that bounds the store.
//!
//! The per-thing EXECUTIONS a job creates are a different store, shared by the same
//! services for the same reason: see [`crate::iot_job_executions`]. A job lives here,
//! what each targeted device is doing about it lives there.
//!
//! Every caller-driven store is BOUNDED: the store holds at most [`MAX_JOBS`] jobs and
//! refuses the create past it ([`IotJobStore::register`]), so no sequence of creates
//! grows it without limit.

use std::collections::BTreeMap;

/// The status of a job that is rolling out to its targets, and the status every
/// creator records: a `JobStatus` value of the IoT model, documented on `Job$status`
/// as one of `IN_PROGRESS`, `CANCELED`, `DELETION_IN_PROGRESS` or `COMPLETED`.
pub const JOB_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The status of a job whose rollout was cancelled. TERMINAL: a cancelled job rolls
/// out to nothing further.
pub const JOB_STATUS_CANCELED: &str = "CANCELED";

/// The status of a job every target has finished. TERMINAL.
pub const JOB_STATUS_COMPLETED: &str = "COMPLETED";

/// The two `JobStatus` values a job never leaves.
pub const TERMINAL_JOB_STATUSES: [&str; 2] = [JOB_STATUS_CANCELED, JOB_STATUS_COMPLETED];

/// Whether `status` is one a job never leaves ([`TERMINAL_JOB_STATUSES`]).
#[must_use]
pub fn is_terminal_job_status(status: &str) -> bool {
    TERMINAL_JOB_STATUSES.contains(&status)
}

/// `targetSelection` for a job that "will be complete after all those things specified
/// as targets have completed the job" (`Job$targetSelection`).
pub const TARGET_SELECTION_SNAPSHOT: &str = "SNAPSHOT";

/// `targetSelection` for a job that "will continue to run" — it "may also be run on a
/// thing when a change is detected in a target" (`Job$targetSelection`), so it is not
/// finished by its current targets finishing.
pub const TARGET_SELECTION_CONTINUOUS: &str = "CONTINUOUS";

/// How many jobs this emulator keeps for the account.
///
/// AWS publishes a service quota on an account's jobs; the exact figure is not in the
/// model, so this is the emulator's own ceiling on a store a caller can grow, and a
/// create past it is refused ([`RegisterRefusal::StoreFull`]) — which each service
/// reports as its own modelled error — rather than growing without bound.
pub const MAX_JOBS: usize = 10_000;

/// One IoT job, as the services that mint and report one persist it.
///
/// It holds what a job NEEDS to be reconstructed by a reader and to create its
/// executions. The members that shape a ROLLOUT rather than the executions themselves
/// (`jobExecutionsRolloutConfig`, `abortConfig`, `jobExecutionsRetryConfig`,
/// `presignedUrlConfig`, `tags`, …) are NOT kept: nothing stages a rollout here, so
/// storing them would promise a later reader data this record does not carry.
/// `targets` IS kept verbatim, so a reader still reports what the job was pointed at.
#[derive(Debug, Clone)]
pub struct IotJob {
    /// The id the job was created under; also its key in the store.
    pub job_id: String,
    /// `arn:<partition>:iot:<region>:<account>:job/<jobId>`, minted on create. It is
    /// the ARN the creating service REPORTED, so the identity a client took from the
    /// create is the identity a reader answers with.
    pub job_arn: String,
    /// The description the job was created with, when it has one.
    pub description: Option<String>,
    /// The job document, when the request carried one. `None` for a job created from
    /// `documentSource`: that names an S3 object this emulator does not fetch, so it
    /// reports no document rather than inventing one.
    pub document: Option<String>,
    /// The `documentSource` the job was created with, when it has one.
    pub document_source: Option<String>,
    /// The targets the job was created with, verbatim.
    pub targets: Vec<String>,
    /// `targetSelection` as supplied ([`TARGET_SELECTION_CONTINUOUS`] /
    /// [`TARGET_SELECTION_SNAPSHOT`]), or `None` when the request left it out.
    pub target_selection: Option<String>,
    /// `timeoutConfig.inProgressTimeoutInMinutes`, carried onto every execution the
    /// job creates.
    pub in_progress_timeout_minutes: Option<i64>,
    /// The `documentParameters` the job was created with: the values its document's
    /// substitution parameters resolve to. Kept so a reader can report the document
    /// BOTH ways — as written, and with every parameter resolved — which is what the
    /// `beforeSubstitution` flag on the job readers selects between.
    pub document_parameters: BTreeMap<String, String>,
    /// The `JobStatus` the job's own lifecycle has reached, as its creator recorded it
    /// and whoever cancelled it last wrote it. What its TARGETS have made of it is not
    /// here: that is counted from the executions
    /// ([`crate::iot_job_executions::JobExecutionStore::executions_of_job`]), so the
    /// two cannot disagree.
    pub status: String,
    /// When the job was created, in epoch seconds.
    pub created_at: i64,
    /// When the job record was last written, in epoch seconds — `Job$lastUpdatedAt`.
    /// Equal to [`Self::created_at`] until something updates the job.
    pub last_updated_at: i64,
    /// `Job$forceCanceled`: "Will be true if the job was canceled with the optional
    /// force parameter set to true." `None` until something cancels the job, so a job
    /// nobody cancelled reports nothing about a force-cancel rather than `false`.
    pub force_canceled: Option<bool>,
    /// The `reasonCode` the call that last moved the job's own lifecycle carried — "a
    /// reason code string that explains why the job was canceled" — or `None` when it
    /// named none.
    pub reason_code: Option<String>,
    /// The `comment` the call that last moved the job's own lifecycle carried — "an
    /// optional comment string describing why the job was canceled" — or `None` when it
    /// named none.
    pub comment: Option<String>,
}

impl IotJob {
    /// A newly created job: [`JOB_STATUS_IN_PROGRESS`], with no document, no targets
    /// and no parameters. A creator overwrites the public fields its own request
    /// carries — the document, the targets, the timeout — which is what keeps every
    /// creator of a job (`iot`, `greengrassv2`) minting the SAME record.
    #[must_use]
    pub fn in_progress(job_id: String, job_arn: String, created_at: i64) -> Self {
        Self {
            job_id,
            job_arn,
            description: None,
            document: None,
            document_source: None,
            targets: Vec::new(),
            target_selection: None,
            in_progress_timeout_minutes: None,
            document_parameters: BTreeMap::new(),
            status: JOB_STATUS_IN_PROGRESS.to_string(),
            created_at,
            last_updated_at: created_at,
            force_canceled: None,
            reason_code: None,
            comment: None,
        }
    }

    /// Whether the job's own status is one it never leaves, so no count of its
    /// executions can move it.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        is_terminal_job_status(&self.status)
    }

    /// Whether the job is a SNAPSHOT job: one that "will be complete after all those
    /// things specified as targets have completed the job"
    /// (`Job$targetSelection`). Only such a job finishes by its targets finishing — a
    /// `CONTINUOUS` one "will continue to run", and a job whose creator named no
    /// selection at all says nothing about either, so neither is reported finished
    /// here.
    #[must_use]
    pub fn is_snapshot(&self) -> bool {
        self.target_selection.as_deref() == Some(TARGET_SELECTION_SNAPSHOT)
    }

    /// Record that the job reached `status` at `now`, which is also the instant the
    /// record was last written.
    pub fn set_status(&mut self, status: &str, now: i64) {
        self.status = status.to_string();
        self.last_updated_at = now;
    }

    /// Record that the job was CANCELLED at `now`: it reaches
    /// [`JOB_STATUS_CANCELED`] — a status it never leaves, so it rolls out to nothing
    /// further — and carries why it was cancelled and whether the cancel was forced.
    ///
    /// The executions the job already created live in
    /// [`crate::iot_job_executions`], and a canceller moves the ones its `force` flag
    /// reaches there
    /// ([`crate::iot_job_executions::JobExecutionStore::cancel_job_executions`]), so
    /// what the JOB says and what each DEVICE's execution says are written in one call.
    pub fn cancel(
        &mut self,
        force: bool,
        reason_code: Option<String>,
        comment: Option<String>,
        now: i64,
    ) {
        self.set_status(JOB_STATUS_CANCELED, now);
        self.force_canceled = Some(force);
        self.reason_code = reason_code;
        self.comment = comment;
    }
}

/// Why a job could not be registered.
///
/// Each variant is a DIFFERENT refusal, so the service that mints the job can report
/// it as the modelled error ITS model declares for that case — a conflict for an id
/// that is taken, a quota failure for a store that is full — rather than one fixed
/// answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterRefusal {
    /// A job already exists under that id: `CreateJob$jobId` is "A job identifier
    /// which must be unique for your account", so the second create is a conflict.
    AlreadyRegistered,
    /// The store already holds [`MAX_JOBS`] jobs.
    StoreFull,
}

impl std::fmt::Display for RegisterRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyRegistered => formatter.write_str("the job already exists"),
            Self::StoreFull => write!(
                formatter,
                "the maximum of {MAX_JOBS} jobs is already reached"
            ),
        }
    }
}

/// The account's IoT jobs, keyed by `jobId` — "A job identifier which must be unique
/// for your account" (`CreateJob$jobId`), which is what makes the id the key and a
/// repeat create a conflict.
///
/// `BTreeMap` (not `HashMap`) because a job lister pages over this store and a page
/// boundary is only meaningful over a stable order.
#[derive(Debug, Default)]
pub struct IotJobStore {
    jobs: BTreeMap<String, IotJob>,
}

impl IotJobStore {
    /// The job `job_id` names, or `None` when no such job exists.
    #[must_use]
    pub fn job(&self, job_id: &str) -> Option<&IotJob> {
        self.jobs.get(job_id)
    }

    /// The job `job_id` names, for a writer that moves its status.
    pub fn job_mut(&mut self, job_id: &str) -> Option<&mut IotJob> {
        self.jobs.get_mut(job_id)
    }

    /// Whether a job exists under `job_id`.
    #[must_use]
    pub fn contains(&self, job_id: &str) -> bool {
        self.jobs.contains_key(job_id)
    }

    /// How many jobs exist.
    #[must_use]
    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    /// Whether no job exists.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// Whether the store is at [`MAX_JOBS`], so the next create is refused.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.jobs.len() >= MAX_JOBS
    }

    /// Record `job`, within the store's bound; `Err` says why a create cannot be
    /// recorded. A refused register writes NOTHING, so a creator that cannot store
    /// the job reports no job rather than an id addressing nothing.
    pub fn register(&mut self, job: IotJob) -> Result<(), RegisterRefusal> {
        if self.jobs.contains_key(&job.job_id) {
            return Err(RegisterRefusal::AlreadyRegistered);
        }
        if self.is_full() {
            return Err(RegisterRefusal::StoreFull);
        }
        self.jobs.insert(job.job_id.clone(), job);
        Ok(())
    }

    /// Forget the job `job_id` names, reporting whether it existed. The job's
    /// EXECUTIONS are not here; a deleter removes those from the execution store.
    pub fn remove(&mut self, job_id: &str) -> bool {
        self.jobs.remove(job_id).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A job as a creator mints one, with the id and ARN it reported.
    fn job(job_id: &str) -> IotJob {
        IotJob::in_progress(
            job_id.to_string(),
            format!("arn:aws:iot:us-east-1:111122223333:job/{job_id}"),
            1_700_000_000,
        )
    }

    /// A freshly minted job is the one every creator records: rolling out, not
    /// terminal, and timestamped once.
    #[test]
    fn a_new_job_is_in_progress() {
        let created = job("deploy-1");
        assert_eq!(
            created.status, JOB_STATUS_IN_PROGRESS,
            "case \"a new job\": expected {JOB_STATUS_IN_PROGRESS}, got {:?}",
            created.status
        );
        assert!(
            !created.is_terminal(),
            "case \"a new job\": a job that is rolling out is not terminal, got {created:?}"
        );
        assert_eq!(
            created.last_updated_at, created.created_at,
            "case \"a new job\": nothing has updated it yet, got {created:?}"
        );
        assert!(
            !created.is_snapshot(),
            "case \"a new job\": a creator that named no targetSelection leaves it \
             unset, got {:?}",
            created.target_selection
        );
    }

    /// Only a registered id names a job, and the record a reader gets is the one the
    /// creator stored.
    #[test]
    fn only_a_registered_id_names_a_job() {
        let mut store = IotJobStore::default();
        assert!(store.is_empty(), "a fresh store holds no job");

        store
            .register(job("deploy-1"))
            .unwrap_or_else(|refusal| panic!("registering the first job: {refusal}"));

        // (case, job id, whether it names a job)
        let rows: [(&str, &str, bool); 4] = [
            ("the registered id", "deploy-1", true),
            ("another id", "deploy-2", false),
            ("the same id in another case", "DEPLOY-1", false),
            ("the id with a suffix", "deploy-10", false),
        ];
        for (case, job_id, expected) in rows {
            let actual = store.contains(job_id);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected contains({job_id:?}) == {expected}, got {actual}"
            );
            assert_eq!(
                store.job(job_id).is_some(),
                expected,
                "case {case:?}: the record of {job_id:?} is readable exactly when it exists"
            );
        }
        assert_eq!(
            store.job("deploy-1").map(|held| held.job_arn.as_str()),
            Some("arn:aws:iot:us-east-1:111122223333:job/deploy-1"),
            "case \"the registered id\": a reader answers the ARN the creator reported"
        );
    }

    /// Registering is bounded and reversible: one id is one job, a cancelled status is
    /// written on the record, and a removed job is gone.
    #[test]
    fn registering_is_bounded_and_reversible() {
        let mut store = IotJobStore::default();
        assert_eq!(store.register(job("deploy-1")), Ok(()), "the first register");
        assert_eq!(
            store.register(job("deploy-1")),
            Err(RegisterRefusal::AlreadyRegistered),
            "case \"a second create under one id\": expected a conflict, got {:?}",
            store.len()
        );

        store
            .job_mut("deploy-1")
            .expect("the job exists")
            .set_status(JOB_STATUS_CANCELED, 1_700_000_100);
        let cancelled = store.job("deploy-1").expect("the job exists");
        assert!(
            cancelled.is_terminal() && cancelled.last_updated_at == 1_700_000_100,
            "case \"a cancelled job\": expected a terminal status stamped at the cancel, \
             got {cancelled:?}"
        );

        assert!(store.remove("deploy-1"), "removing it reports it existed");
        assert!(
            !store.remove("deploy-1"),
            "removing it twice reports it was already gone"
        );

        for index in 0..MAX_JOBS {
            let job_id = format!("bulk-{index}");
            assert_eq!(
                store.register(job(&job_id)),
                Ok(()),
                "case {job_id:?}: job {index} of the bound must be registrable"
            );
        }
        assert_eq!(
            store.register(job("over-the-bound")),
            Err(RegisterRefusal::StoreFull),
            "case \"one job past the bound\": expected a refusal, got len {}",
            store.len()
        );
        assert_eq!(
            store.len(),
            MAX_JOBS,
            "a refused register stores nothing, got len {}",
            store.len()
        );
    }

    /// RULE: cancelling a job moves it to the terminal `CANCELED` status at the instant
    /// of the cancel, and records whether the cancel was forced and what it stated about
    /// itself — which is what a reader of the job then reports.
    ///
    /// The rows span the force flag and whether the cancel stated a reason, a comment,
    /// both or neither.
    #[test]
    fn cancelling_a_job_records_the_cancel_on_the_record() {
        // (case, force, reasonCode, comment)
        let rows: [(&str, bool, Option<&str>, Option<&str>); 4] = [
            ("an unforced cancel stating nothing", false, None, None),
            ("a forced cancel stating a reason", true, Some("RECALLED"), None),
            (
                "an unforced cancel stating a comment",
                false,
                None,
                Some("no longer needed"),
            ),
            (
                "a forced cancel stating both",
                true,
                Some("SUPERSEDED_1"),
                Some("replaced by the next rollout"),
            ),
        ];

        for (case, force, reason_code, comment) in rows {
            let mut cancelled = job("deploy-1");
            assert!(
                cancelled.force_canceled.is_none()
                    && cancelled.reason_code.is_none()
                    && cancelled.comment.is_none(),
                "case {case:?}: a job nobody cancelled states nothing about a cancel, \
                 got {cancelled:?}"
            );

            cancelled.cancel(
                force,
                reason_code.map(str::to_string),
                comment.map(str::to_string),
                1_700_000_500,
            );
            assert_eq!(
                (cancelled.status.as_str(), cancelled.last_updated_at),
                (JOB_STATUS_CANCELED, 1_700_000_500),
                "case {case:?}: expected {JOB_STATUS_CANCELED} stamped at the cancel, \
                 got {cancelled:?}"
            );
            assert!(
                cancelled.is_terminal(),
                "case {case:?}: a cancelled job rolls out to nothing further, got {cancelled:?}"
            );
            assert_eq!(
                (
                    cancelled.force_canceled,
                    cancelled.reason_code.as_deref(),
                    cancelled.comment.as_deref()
                ),
                (Some(force), reason_code, comment),
                "case {case:?}: expected the cancel's own members on the record, \
                 got {cancelled:?}"
            );
            assert_eq!(
                cancelled.created_at, 1_700_000_000,
                "case {case:?}: a cancel never moves when the job was created, \
                 got {cancelled:?}"
            );
        }
    }
}
