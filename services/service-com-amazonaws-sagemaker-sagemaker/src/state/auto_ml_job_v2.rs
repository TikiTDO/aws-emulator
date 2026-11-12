// SPDX-License-Identifier: CC0-1.0

//! The AutoML job V2 slice of this service's state: what `CreateAutoMLJobV2` persists
//! and `DescribeAutoMLJobV2` replays.
//!
//! A child module of [`crate::state`] rather than another map beside it: the record,
//! the constants that bound and word it and the derived `AutoMLProblemTypeConfigName`
//! live here, while the ONE state struct keeps the single
//! [`auto_ml_jobs_v2`](super::ComAmazonawsSagemakerSagemakerState::auto_ml_jobs_v2) map
//! every AutoML operation reads, and [`crate::state`] re-exports this module's items so
//! a handler names them as `crate::state::…` like the rest of the slice.
//!
//! # The job's status is moved by the CLOCK, never by a read
//!
//! AWS returns an accepted AutoML job in the non-terminal `InProgress` status, so
//! leaving it is owed by the clock: the due instant is stored at create time and
//! [`AutoMLJobV2::settle_status`] only materialises what is already due, so a read never
//! changes what a later caller sees and the transient status is observable because the
//! span is never zero.

use std::time::{Duration, SystemTime};

// The modelled shapes this slice keeps, reached through the parent module's import list
// rather than imported again here: `crate::state`'s `use crate::ops::types::{…}` is the
// list every concurrent change to this service edits, so naming these through `super`
// leaves it exactly as it was.
use super::{
    AutoMLComputeConfig, AutoMLDataSplitConfig, AutoMLJobChannel, AutoMLJobObjective,
    AutoMLOutputDataConfig, AutoMLProblemTypeConfig, AutoMLSecurityConfig, ModelDeployConfig, Tag,
};

/// See [`super::MODEL_QUOTA`]. AutoML jobs are not deleted through the API, so this is
/// what bounds the job store: a caller varying `AutoMLJobName` cannot grow it without
/// end.
pub const AUTO_ML_JOB_QUOTA: usize = 1_000;

/// How long after the stage before it the clock owes an AutoML job its next stage of work.
///
/// AWS documents no duration for an Autopilot job, so the span is this emulator's own,
/// compressed and never zero: a caller that describes the job straight after creating it
/// still observes `InProgress`/`Starting`, and one that keeps polling observes each stage of
/// the work in turn before the terminal status. Nothing sleeps: the due instant is stored and
/// materialised by the next call that reads the record.
pub const AUTO_ML_JOB_PHASE_SPAN: Duration = Duration::from_millis(700);

/// How long after `CreateAutoMLJobV2` the clock owes the job's terminal transition.
///
/// The whole of the work: the `Starting` the job is accepted in, then the stages a run gets
/// through ([`AutoMLRunOutcome::phases`]), each owed [`AUTO_ML_JOB_PHASE_SPAN`] after the one
/// before it. This is the span of a run that reads its data, fits and measures its candidates
/// and writes their artifacts — the longest a job here takes — so a caller that waits this
/// long observes a terminal status and a waiter terminates.
pub const AUTO_ML_JOB_TERMINAL_AFTER: Duration = AUTO_ML_JOB_PHASE_SPAN.saturating_mul(5);

/// `AutoMLJobStatus`'s members an AutoML job V2 created here passes through
/// (`smithy_query` shape=AutoMLJobStatus: Completed | InProgress | Failed | Stopped |
/// Stopping — the stop states belong to `StopAutoMLJob`, still a stub).
pub const AUTO_ML_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`AUTO_ML_STATUS_IN_PROGRESS`]: the status of a job whose run finished.
pub const AUTO_ML_STATUS_COMPLETED: &str = "Completed";
/// See [`AUTO_ML_STATUS_IN_PROGRESS`].
pub const AUTO_ML_STATUS_FAILED: &str = "Failed";
/// `AutoMLJobSecondaryStatus`'s members an AutoML job V2 created here passes through
/// (`smithy_query` shape=AutoMLJobSecondaryStatus), each named once here so no handler
/// compares a bare string literal: the job has started, the data is being analysed, it is
/// being transformed, the model is being tuned, the candidate definitions have been written,
/// and then the member that goes with the job's terminal status.
pub const AUTO_ML_SECONDARY_STATUS_STARTING: &str = "Starting";
/// See [`AUTO_ML_SECONDARY_STATUS_STARTING`].
pub const AUTO_ML_SECONDARY_STATUS_ANALYZING_DATA: &str = "AnalyzingData";
/// See [`AUTO_ML_SECONDARY_STATUS_STARTING`].
pub const AUTO_ML_SECONDARY_STATUS_FEATURE_ENGINEERING: &str = "FeatureEngineering";
/// See [`AUTO_ML_SECONDARY_STATUS_STARTING`].
pub const AUTO_ML_SECONDARY_STATUS_MODEL_TUNING: &str = "ModelTuning";
/// See [`AUTO_ML_SECONDARY_STATUS_STARTING`].
pub const AUTO_ML_SECONDARY_STATUS_CANDIDATE_DEFINITIONS_GENERATED: &str =
    "CandidateDefinitionsGenerated";
/// See [`AUTO_ML_SECONDARY_STATUS_STARTING`]: the member that goes with
/// [`AUTO_ML_STATUS_COMPLETED`].
pub const AUTO_ML_SECONDARY_STATUS_COMPLETED: &str = "Completed";
/// See [`AUTO_ML_SECONDARY_STATUS_STARTING`]: the member that goes with
/// [`AUTO_ML_STATUS_FAILED`].
pub const AUTO_ML_SECONDARY_STATUS_FAILED: &str = "Failed";

/// The kind label [`super::could_not_find`] names an AutoML job V2 by.
pub const AUTO_ML_JOB_KIND_LABEL: &str = "AutoML job";

/// `FailureReason` of an AutoML job V2 whose run never reported at all — the request that
/// started it did not get to the end of its own handler, so nothing is known about the work.
///
/// The class AWS reports for a failure on its own side rather than in the caller's
/// configuration, so a client classifies it as retryable. A run that DID report says what it
/// found instead (see [`super::auto_ml_run`]).
pub const AUTO_ML_INTERNAL_FAILURE_REASON: &str = super::INTERNAL_FAILURE_REASON;

/// An AutoML job V2 created by `CreateAutoMLJobV2`, as `DescribeAutoMLJobV2` replays it.
///
/// Every request member the job was created with is kept, so the describe replays what
/// was stored rather than a reconstruction, and `terminal_at` is the instant the clock
/// owes the move out of `InProgress` — into the status the job's own RUN reported.
#[derive(Debug, Clone)]
pub struct AutoMLJobV2 {
    /// The name as the caller spelled it. The lookup key is
    /// [`super::entity_key`]'s case-folded form of it.
    pub name: String,
    pub arn: String,
    pub input_data_config: Vec<AutoMLJobChannel>,
    pub output_data_config: AutoMLOutputDataConfig,
    pub problem_type_config: AutoMLProblemTypeConfig,
    pub role_arn: String,
    pub objective: Option<AutoMLJobObjective>,
    pub model_deploy_config: Option<ModelDeployConfig>,
    pub data_split_config: Option<AutoMLDataSplitConfig>,
    pub security_config: Option<AutoMLSecurityConfig>,
    pub compute_config: Option<AutoMLComputeConfig>,
    pub tags: Vec<Tag>,
    /// An `AutoMLJobStatus` value.
    pub status: String,
    /// An `AutoMLJobSecondaryStatus` value.
    pub secondary_status: String,
    pub failure_reason: Option<String>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub end_time: Option<SystemTime>,
    pub terminal_at: SystemTime,
    /// What the job's RUN reported ([`super::run_auto_ml_job_v2`]), recorded by the create
    /// that started it and revealed by the clock ([`Self::settle_status`]). `None` while the
    /// run has not reported.
    pub run: Option<super::AutoMLRunOutcome>,
    /// The `AutoMLJobSecondaryStatus` stages this job's work passes through, in order and each
    /// with the instant the job enters it: `Starting` from the create, then the stages the run
    /// reported, laid out over the span the clock gives the work. Stored, never slept on.
    pub phases: Vec<AutoMLPhase>,
    /// How many of [`Self::phases`] the clock has already materialised — at least the
    /// `Starting` the job was accepted in, so a read never reports a stage that is not due yet
    /// and never un-reports one that is.
    pub entered: usize,
}

/// One stage of an AutoML job's work: the `AutoMLJobSecondaryStatus` member the job is in and
/// the instant it enters it.
#[derive(Debug, Clone)]
pub struct AutoMLPhase {
    /// An `AutoMLJobSecondaryStatus` member.
    pub status: String,
    /// When the job enters it — stored, and materialised by [`AutoMLJobV2::settle_status`]
    /// once it comes due.
    pub starts_at: SystemTime,
}

impl AutoMLJobV2 {
    /// Materialise the move out of `InProgress` the clock already owes.
    ///
    /// The one write a reporting operation may make: the transition was due at `terminal_at`
    /// whether or not anybody described the job, so the observer makes no difference to the
    /// result. `LastModifiedTime` and `EndTime` move to the instant the transition was due,
    /// not to "now", so both stay stable across later reads.
    ///
    /// The terminal status is the one the job's own RUN reported ([`Self::run`]) — `Completed`
    /// with the candidates it fitted and the notebooks it wrote, or `Failed` with the
    /// condition that stopped it. A job whose run has NOT reported yet is not settled at all,
    /// whatever the clock says: the terminal status IS the run's, and [`Self::record_run`]
    /// pushes the due instant out to the moment it reports, so no reader can turn a run that
    /// is still going into a failure.
    ///
    /// The stages of the work the clock has already made due are materialised on the same
    /// terms, in the order the run went through them, so a caller polling a running job
    /// observes the analysis, the feature engineering and the tuning before the terminal
    /// status and never a stage out of order.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status != AUTO_ML_STATUS_IN_PROGRESS {
            return false;
        }
        let moved = self.enter_phases_due(now.min(self.terminal_at));
        if now < self.terminal_at {
            return moved;
        }
        match self.run.as_ref() {
            Some(super::AutoMLRunOutcome::Completed(_)) => {
                self.status = AUTO_ML_STATUS_COMPLETED.to_string();
                self.secondary_status = AUTO_ML_SECONDARY_STATUS_COMPLETED.to_string();
            }
            Some(super::AutoMLRunOutcome::Failed { reason, .. }) => {
                self.status = AUTO_ML_STATUS_FAILED.to_string();
                self.secondary_status = AUTO_ML_SECONDARY_STATUS_FAILED.to_string();
                self.failure_reason = Some(reason.clone());
            }
            // The run has not reported: nothing is owed until it does.
            None => return moved,
        }
        self.entered = self.phases.len();
        self.last_modified_time = self.terminal_at;
        self.end_time = Some(self.terminal_at);
        true
    }

    /// Materialise every stage of the work the clock has made due by `now`, in order.
    ///
    /// Answers whether any of them moved on this call.
    fn enter_phases_due(&mut self, now: SystemTime) -> bool {
        let mut moved = false;
        while let Some(phase) = self
            .phases
            .get(self.entered)
            .filter(|phase| phase.starts_at <= now)
            .cloned()
        {
            self.secondary_status = phase.status;
            self.last_modified_time = phase.starts_at;
            self.entered += 1;
            moved = true;
        }
        moved
    }

    /// Record what the job's RUN reported, `at` the instant it reported.
    ///
    /// The clock owes the job its terminal status [`AUTO_ML_JOB_TERMINAL_AFTER`] after the
    /// create — but it cannot owe it before the work is over, so a run that took longer than
    /// that span pushes the due instant out to the moment it reported. The status the clock
    /// then materialises is always the one the run produced.
    ///
    /// The stages the run went through are laid out over the span the clock gives the work,
    /// each owed [`AUTO_ML_JOB_PHASE_SPAN`] after the one before it, so the job enters them in
    /// the order the run performed them and the terminal status falls due after the last of
    /// them.
    pub fn record_run(&mut self, outcome: super::AutoMLRunOutcome, at: SystemTime) {
        if self.status == AUTO_ML_STATUS_IN_PROGRESS {
            let reached = outcome.phases().to_vec();
            for (index, status) in reached.iter().enumerate() {
                self.phases.push(AutoMLPhase {
                    status: status.clone(),
                    starts_at: self.creation_time
                        + AUTO_ML_JOB_PHASE_SPAN
                            .saturating_mul(u32::try_from(index + 1).unwrap_or(u32::MAX)),
                });
            }
            let scheduled = self.creation_time
                + AUTO_ML_JOB_PHASE_SPAN
                    .saturating_mul(u32::try_from(reached.len() + 1).unwrap_or(u32::MAX));
            self.terminal_at = scheduled.max(at);
        }
        self.run = Some(outcome);
    }

    /// What this job's run FINISHED with, once the clock has materialised its terminal status
    /// — and `None` while the job is still running or its run failed, because a reader reports
    /// a result only once the job holds one.
    #[must_use]
    pub fn result(&self) -> Option<&super::AutoMLRunResult> {
        if self.status != AUTO_ML_STATUS_COMPLETED {
            return None;
        }
        match self.run.as_ref() {
            Some(super::AutoMLRunOutcome::Completed(result)) => Some(result),
            _ => None,
        }
    }
}

impl super::ComAmazonawsSagemakerSagemakerState {
    /// Whether another AutoML job V2 fits, i.e. the store is below
    /// [`AUTO_ML_JOB_QUOTA`]. Beside the state struct's other `*_below_quota` readers.
    pub fn auto_ml_jobs_v2_below_quota(&self) -> bool {
        self.auto_ml_jobs_v2.len() < AUTO_ML_JOB_QUOTA
    }
}

/// The `AutoMLProblemTypeConfigName` that names which `AutoMLProblemTypeConfig` union
/// member a job was created with (`smithy_query` shape=AutoMLProblemTypeConfigName:
/// ImageClassification | TextClassification | TimeSeriesForecasting | Tabular |
/// TextGeneration). Derived from the stored config, so the two can never disagree.
pub fn auto_ml_problem_type_config_name(config: &AutoMLProblemTypeConfig) -> &'static str {
    match config {
        AutoMLProblemTypeConfig::ImageClassificationJobConfig(_) => "ImageClassification",
        AutoMLProblemTypeConfig::TextClassificationJobConfig(_) => "TextClassification",
        AutoMLProblemTypeConfig::TimeSeriesForecastingJobConfig(_) => "TimeSeriesForecasting",
        AutoMLProblemTypeConfig::TabularJobConfig(_) => "Tabular",
        AutoMLProblemTypeConfig::TextGenerationJobConfig(_) => "TextGeneration",
    }
}
