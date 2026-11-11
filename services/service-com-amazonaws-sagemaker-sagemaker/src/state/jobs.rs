// SPDX-License-Identifier: CC0-1.0

//! The JOB slice of this service's state: the records a training, hyper-parameter
//! tuning, trial, processing or inference-recommendations job keeps, with the clock
//! spans and statuses that belong to them.
//!
//! Moved out of [`super`] unchanged, so the parent module stays inside the project's
//! file-length limit and each job record reads beside the clock that moves it. Every
//! name is re-exported from [`super`], so a handler keeps naming `crate::state::…`.

use super::*;

/// How long after `CreateTrainingJob` the clock owes the job's move out of `InProgress`
/// into its terminal status.
///
/// Same contract as [`PROCESSING_JOB_ENDS_AFTER`], and chosen on the same terms (AWS
/// documents no duration for a training run): compressed, never zero, so a caller that
/// describes the job straight after creating it still observes `InProgress` with the
/// secondary status of a job whose work has not started, and one that waits (as `aws
/// sagemaker wait training-job-completed-or-stopped` does) observes a terminal status, so
/// a waiter terminates.
pub const TRAINING_JOB_ENDS_AFTER: Duration = Duration::from_millis(400);

/// `TrainingJobStatus`'s members this emulator moves a training job through (the modelled
/// enum also carries `Deleting`, which this slice does not reach: `DeleteTrainingJob` is
/// still a stub). Named once here so no handler compares a bare string literal.
pub const TRAINING_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`TRAINING_STATUS_IN_PROGRESS`]: the status of a job whose run finished.
pub const TRAINING_STATUS_COMPLETED: &str = "Completed";
/// See [`TRAINING_STATUS_IN_PROGRESS`].
pub const TRAINING_STATUS_FAILED: &str = "Failed";
/// See [`TRAINING_STATUS_IN_PROGRESS`]: the transient status a job carries from the
/// `StopTrainingJob` call until the clock owes it [`TRAINING_STATUS_STOPPED`] ("When it
/// receives a StopTrainingJob request, SageMaker changes the status of the job to
/// Stopping" — that operation's own doc).
pub const TRAINING_STATUS_STOPPING: &str = "Stopping";
/// See [`TRAINING_STATUS_STOPPING`]: the terminal status of a job the caller stopped
/// ("After SageMaker stops the job, it sets the status to Stopped").
pub const TRAINING_STATUS_STOPPED: &str = "Stopped";

/// The `SecondaryStatus` an accepted training job carries: the job has been accepted and
/// no stage of its work has begun.
pub const TRAINING_SECONDARY_STATUS_STARTING: &str = "Starting";
/// See [`TRAINING_SECONDARY_STATUS_STARTING`]: the member that goes with
/// [`TRAINING_STATUS_COMPLETED`] (`SecondaryStatus` spells it exactly as
/// `TrainingJobStatus` does).
pub const TRAINING_SECONDARY_STATUS_COMPLETED: &str = "Completed";
/// See [`TRAINING_SECONDARY_STATUS_STARTING`]: the member that goes with
/// [`TRAINING_STATUS_FAILED`].
pub const TRAINING_SECONDARY_STATUS_FAILED: &str = "Failed";
/// See [`TRAINING_SECONDARY_STATUS_STARTING`]: the member that goes with
/// [`TRAINING_STATUS_STOPPING`] (`SecondaryStatus` carries `Stopping` too, spelled the
/// same way `TrainingJobStatus` spells it).
pub const TRAINING_SECONDARY_STATUS_STOPPING: &str = "Stopping";
/// See [`TRAINING_SECONDARY_STATUS_STARTING`]: the member that goes with
/// [`TRAINING_STATUS_STOPPED`].
pub const TRAINING_SECONDARY_STATUS_STOPPED: &str = "Stopped";

/// How long after `StopTrainingJob` the clock owes the job's `Stopping` -> `Stopped` move.
///
/// Chosen on the same terms as [`TRAINING_JOB_ENDS_AFTER`] and inside the same band this
/// crate uses for every clock-owed span (see [`PROCESSING_JOB_STOPS_AFTER`] and
/// [`TUNING_JOB_STOPS_AFTER`]): the 120 seconds the operation's doc mentions is the grace
/// window SageMaker gives the ALGORITHM to save its artifacts, not a wait the API contract
/// states for the caller, so the span here is compressed like every other one. It is never
/// zero, so a caller that describes the job straight after asking it to stop still observes
/// the transient `Stopping`, and one that waits observes `Stopped`, so a waiter terminates.
pub const TRAINING_JOB_STOPS_AFTER: Duration = Duration::from_millis(300);

/// Why stopping a training job whose run is already over is refused, naming the job and the
/// status it had already reached.
///
/// A job in a status nothing leaves on its own has no run left to stop. `StopTrainingJob`
/// models only `ResourceNotFound` and the job IS found, so the refusal is a 400
/// `ValidationException` carrying this sentence — the form this crate's other stops use
/// (see [`crate::state::processing_job_not_running`]).
#[must_use]
pub fn training_job_not_running(name: &str, status: &str) -> String {
    format!(
        "The training job \"{name}\" is {status} and is not running, so there is \
         nothing to stop."
    )
}

/// `FailureReason` of a training job whose run never reported at all — the request that
/// started it did not get to the end of its own handler, so nothing is known about the work.
///
/// The class AWS reports for a failure on its own side rather than in the caller's
/// configuration, so a client classifies it as retryable. A run that DID report says what it
/// found instead (see [`crate::state::training_run`]).
pub const INTERNAL_FAILURE_REASON: &str =
    "InternalServerError: We encountered an internal error. Please try again.";

/// A training job created by `CreateTrainingJob`, as `DescribeTrainingJob` replays it and
/// `ListTrainingJobs` summarises it.
///
/// One record per `TrainingJobName` (the name AWS keys a training job by, unique within an
/// account and Region). The members a `TrainingJobSummary` projects are fields of their
/// own, because the listing sorts and filters on them; everything else the describe
/// reports is the accepted request, kept verbatim.
///
/// The store key is the name AS SENT, not folded through [`entity_key`]: `TrainingJobName`
/// is documented unique "within an Amazon Web Services Region in an Amazon Web Services
/// account" and nothing in the model makes it case-insensitive, where
/// `HyperParameterTuningJobName`'s own doc says in so many words that it "is not case
/// sensitive" — which is why that family folds its key and this one does not.
///
/// `ends_at` is the instant the clock owes the job's move out of `InProgress`; it is
/// stored, never slept on, and materialised by whichever reporting call first runs at or
/// after it ([`TrainingJob::settle_status`]).
#[derive(Debug, Clone)]
pub struct TrainingJob {
    pub name: String,
    pub arn: String,
    /// A `TrainingJobStatus` member (`InProgress` | `Completed` | `Failed` | `Stopping` |
    /// `Stopped` | `Deleting`), as stored — never derived from the read that reports it.
    pub status: String,
    /// A `SecondaryStatus` member, when the job has one.
    pub secondary_status: Option<String>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    /// Set only once the job reached a terminal status, per `TrainingJobSummary`'s
    /// `TrainingEndTime` doc ("set only if the training job has one of the terminal
    /// statuses").
    pub training_end_time: Option<SystemTime>,
    /// The warm pool associated with the job, when it has one.
    pub warm_pool_status: Option<crate::ops::types::WarmPoolStatus>,
    /// The training plan the job reserves capacity from, when it has one.
    pub training_plan_arn: Option<String>,
    /// Why the job is in a terminal failure status, once it reached one.
    pub failure_reason: Option<String>,
    /// What the job's RUN reported ([`crate::state::run_training_job`]), recorded by the
    /// create that started it and revealed by the clock ([`Self::settle_status`]). `None`
    /// while the run has not reported.
    pub run: Option<crate::state::TrainingRunOutcome>,
    /// "Indicates the time when the training job starts on training instances"
    /// (`DescribeTrainingJob$TrainingStartTime`): set once the clock has settled the job,
    /// because the run's own work begins after the job is accepted.
    pub training_start_time: Option<SystemTime>,
    /// The object the run wrote the model artifacts to, once it wrote one.
    pub model_artifacts: Option<String>,
    /// "The name, value, and date and time of a metric that was emitted" by the run, once
    /// it reported any.
    pub final_metrics: Vec<crate::ops::types::MetricData>,
    /// When the clock owes this job its move out of `InProgress`.
    pub ends_at: SystemTime,
    /// The hyperparameter tuning job that launched this training job, when one did
    /// (`DescribeTrainingJob$TuningJobArn`). A training job a caller creates has no
    /// parent, and nothing here launches one on a tuning job's behalf — which is why
    /// [`ComAmazonawsSagemakerSagemakerState::parallel_training_jobs_in_use`] counts none.
    pub tuning_job_arn: Option<String>,
    /// The `CreateTrainingJob` request the job was accepted with, kept verbatim so
    /// `DescribeTrainingJob` replays what was stored rather than a reconstruction of it.
    pub request: crate::ops::types::CreateTrainingJobInput,
    /// The request's `Tags`, projected out of [`Self::request`] so the one tag resolution
    /// (`tag_accessors!`) reaches this record's tag set through the `tags` field every
    /// other taggable record exposes.
    pub tags: Vec<Tag>,
}

impl TrainingJob {
    /// Materialise the move out of a non-terminal status the clock already owes this job.
    ///
    /// This is the one write a reporting operation may make: the transition was due at
    /// `ends_at` whether or not anybody described the job, so the observer makes no
    /// difference to the result. For a job left to run the terminal status is the one the
    /// job's own RUN reported ([`Self::run`]) — `Completed` with the artifact it wrote and
    /// the metrics it measured, or `Failed` with the condition that stopped it. For a job the
    /// caller asked to stop it is `Stopped`, with no failure reason beside it: stopping is
    /// what the caller asked for, not a failure, and whatever the abandoned run reported is
    /// not revealed as the job's own outcome.
    ///
    /// A job still RUNNING whose run has NOT reported yet is not settled at all, whatever the
    /// clock says: the terminal status IS the run's, so there is none to materialise until
    /// the run has one, and [`Self::record_run`] pushes the due instant out to the moment it
    /// reports. So no reader can turn a run that is still going into a failure, and a run
    /// slower than [`TRAINING_JOB_ENDS_AFTER`] still settles into what it actually produced.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if now < self.ends_at {
            return false;
        }
        match self.status.as_str() {
            TRAINING_STATUS_IN_PROGRESS => match self.run.clone() {
                Some(crate::state::TrainingRunOutcome::Completed {
                    model_artifacts,
                    final_metrics,
                }) => {
                    self.status = TRAINING_STATUS_COMPLETED.to_string();
                    self.secondary_status = Some(TRAINING_SECONDARY_STATUS_COMPLETED.to_string());
                    self.model_artifacts = Some(model_artifacts);
                    self.final_metrics = final_metrics;
                }
                Some(crate::state::TrainingRunOutcome::Failed { reason }) => {
                    self.status = TRAINING_STATUS_FAILED.to_string();
                    self.secondary_status = Some(TRAINING_SECONDARY_STATUS_FAILED.to_string());
                    self.failure_reason = Some(reason);
                }
                // The run has not reported: nothing is owed until it does.
                None => return false,
            },
            TRAINING_STATUS_STOPPING => {
                self.status = TRAINING_STATUS_STOPPED.to_string();
                self.secondary_status = Some(TRAINING_SECONDARY_STATUS_STOPPED.to_string());
            }
            // Already terminal: nothing is owed and nothing moves.
            _ => return false,
        }
        // The run's work begins once the job is accepted and is over by the instant the
        // clock settles it, which is the span the describe reports.
        self.training_start_time = Some(self.creation_time);
        self.last_modified_time = self.ends_at;
        self.training_end_time = Some(self.ends_at);
        true
    }

    /// Record what the job's RUN reported, `at` the instant it reported.
    ///
    /// The clock owes the job its terminal status [`TRAINING_JOB_ENDS_AFTER`] after the create
    /// — but it cannot owe it before the work is over, so a run that took longer than that
    /// span pushes the due instant out to the moment it reported. The status the clock then
    /// materialises is always the one the run produced.
    ///
    /// A job the caller has already asked to stop keeps the due instant that stop set: the
    /// terminal status it is waiting for is `Stopped`, which does not depend on what the
    /// abandoned run reported, so a late report must not push `Stopped` further away.
    pub fn record_run(&mut self, outcome: crate::state::TrainingRunOutcome, at: SystemTime) {
        self.run = Some(outcome);
        if self.status == TRAINING_STATUS_IN_PROGRESS && at > self.ends_at {
            self.ends_at = at;
        }
    }

    /// Whether the job has reached a status nothing leaves on its own, i.e. one of
    /// `TrainingJobStatus`'s terminal members.
    ///
    /// `Deleting` belongs to `DeleteTrainingJob`, which this slice does not reach, so the
    /// terminal statuses are the three a job here can be in.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            TRAINING_STATUS_COMPLETED | TRAINING_STATUS_FAILED | TRAINING_STATUS_STOPPED
        )
    }

    /// Whether a stop the caller already asked for is still in flight, i.e. the job sits in
    /// the transient `Stopping` status.
    pub fn is_stopping(&self) -> bool {
        self.status == TRAINING_STATUS_STOPPING
    }

    /// Record that the caller asked this job to stop: it moves to the transient `Stopping`
    /// now, and the clock owes it `Stopped` at [`TRAINING_JOB_STOPS_AFTER`]. The span is
    /// never zero, so the transient status is observable.
    ///
    /// The run the job was doing is abandoned rather than reported: nothing is published
    /// beside `Stopping` or the `Stopped` that follows it, so no failure reason, artifact or
    /// metric from a run the caller cut short is revealed as the job's own outcome.
    pub fn request_stop(&mut self, now: SystemTime) {
        self.status = TRAINING_STATUS_STOPPING.to_string();
        self.secondary_status = Some(TRAINING_SECONDARY_STATUS_STOPPING.to_string());
        self.failure_reason = None;
        self.training_end_time = None;
        self.last_modified_time = now;
        self.ends_at = now + TRAINING_JOB_STOPS_AFTER;
    }

    /// "The training time in seconds" (`DescribeTrainingJob$TrainingTimeInSeconds`): the
    /// span between the instant the run started and the instant it ended, rounded UP to the
    /// whole seconds AWS reports, and `None` for a job that has not ended.
    pub fn training_time_in_seconds(&self) -> Option<i32> {
        let started = self.training_start_time?;
        let ended = self.training_end_time?;
        let span = ended.duration_since(started).unwrap_or(Duration::ZERO);
        let seconds = span.as_secs() + u64::from(span.subsec_nanos() > 0);
        i32::try_from(seconds).ok()
    }

    /// The object `ModelArtifacts$S3ModelArtifacts` reports: the one the run WROTE once it
    /// has, and otherwise the location it would write to ([`Self::model_artifacts_path`]).
    pub fn reported_model_artifacts(&self) -> String {
        self.model_artifacts
            .clone()
            .unwrap_or_else(|| self.model_artifacts_path())
    }

    /// Where the job's model artifacts are written, from the output location the request
    /// named and the job's own name.
    ///
    /// `ModelArtifacts$S3ModelArtifacts` is "the path of the S3 object that contains the
    /// model artifacts", and SageMaker writes it under `<S3OutputPath>/<TrainingJobName>`;
    /// it is DERIVED from the request rather than a report that an object exists, and
    /// stays the same across reads for as long as the job is held.
    pub fn model_artifacts_path(&self) -> String {
        let base = self
            .request
            .output_data_config
            .s3_output_path
            .trim_end_matches('/');
        format!(
            "{base}/{}/output/{}",
            self.name,
            crate::state::MODEL_ARCHIVE_OBJECT
        )
    }

    /// "Profiling status of a training job" (`ProfilingStatus`), which follows the
    /// profiler configuration the job was accepted with: a job is profiled only where a
    /// `ProfilerConfig` asked for it and did not switch it off, so a job created with no
    /// profiler configuration at all — which is then reported with none — is not being
    /// profiled.
    pub fn profiling_status(&self) -> String {
        let profiled = self
            .request
            .profiler_config
            .as_ref()
            .is_some_and(|config| !config.disable_profiler.unwrap_or(false));
        if profiled {
            PROFILING_STATUS_ENABLED.to_string()
        } else {
            PROFILING_STATUS_DISABLED.to_string()
        }
    }

    /// "A history of all of the secondary statuses that the training job has transitioned
    /// through" (`SecondaryStatusTransitions`): one entry per secondary status this job
    /// has actually been in, each starting when the job entered it and ending when it
    /// left, built from the instants the record holds rather than from any history kept
    /// beside them.
    ///
    /// A job still in its first secondary status reports that one entry, open-ended; the one
    /// it has moved on to starts where the first ended, ends when the job's own run ended —
    /// so it stays open-ended while a stop is in flight — and carries the reason the job
    /// reports, so the history says the same thing the job's own `FailureReason` does.
    pub fn secondary_status_transitions(&self) -> Vec<SecondaryStatusTransition> {
        let current = self
            .secondary_status
            .as_deref()
            .unwrap_or(TRAINING_SECONDARY_STATUS_STARTING);
        // The job left `Starting` when it entered the status it is in now, which is the
        // instant that move last modified the record — the clock settling the job, or the
        // stop the caller asked for.
        let left_starting =
            (current != TRAINING_SECONDARY_STATUS_STARTING).then_some(self.last_modified_time);
        let mut transitions = vec![SecondaryStatusTransition {
            status: TRAINING_SECONDARY_STATUS_STARTING.to_string(),
            start_time: timestamp(self.creation_time),
            end_time: left_starting.map(timestamp),
            status_message: None,
        }];
        if let Some(at) = left_starting {
            transitions.push(SecondaryStatusTransition {
                status: current.to_string(),
                start_time: timestamp(at),
                end_time: self.training_end_time.map(timestamp),
                status_message: self.failure_reason.clone(),
            });
        }
        transitions
    }
}

/// The two `ProfilingStatus` members: a training job is profiled only where a
/// `ProfilerConfig` asked for it.
pub const PROFILING_STATUS_ENABLED: &str = "Enabled";
/// See [`PROFILING_STATUS_ENABLED`].
pub const PROFILING_STATUS_DISABLED: &str = "Disabled";

/// The hyperparameters a training job KEEPS, out of those its create supplied.
///
/// `HyperParameterValue` is modelled `@length min 0` — an empty value is a legal thing to
/// send — but recorded SageMaker does not keep an entry whose value is the empty string:
/// the described job reports a map without that key, so a key supplied empty is
/// indistinguishable from one never supplied, while every non-empty value round-trips
/// unchanged. A caller that reads the map back to decide whether a knob was configured
/// therefore sees the same key set here as on the service.
///
/// Applied where the job is STORED rather than where one reader reports it, so every
/// reader of the record answers the same key set.
pub fn retained_hyper_parameters(
    supplied: Option<HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    supplied.map(|supplied| {
        supplied
            .into_iter()
            .filter(|(_, value)| !value.is_empty())
            .collect()
    })
}

/// A hyperparameter tuning job created by `CreateHyperParameterTuningJob`, as
/// `DescribeHyperParameterTuningJob` replays it.
///
/// `settles_at` is the instant the clock owes the `InProgress` -> terminal transition;
/// it is stored, never slept on, and materialised by whichever reporting call first
/// runs at or after it.
#[derive(Debug, Clone)]
pub struct HyperParameterTuningJob {
    pub name: String,
    pub arn: String,
    pub config: HyperParameterTuningJobConfig,
    pub training_job_definition: Option<HyperParameterTrainingJobDefinition>,
    pub training_job_definitions: Option<Vec<HyperParameterTrainingJobDefinition>>,
    pub warm_start_config: Option<HyperParameterTuningJobWarmStartConfig>,
    pub autotune: Option<Autotune>,
    pub status: String,
    pub failure_reason: Option<String>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    /// `HyperParameterTuningEndTime`: set when the job reaches a terminal status.
    pub end_time: Option<SystemTime>,
    pub settles_at: SystemTime,
}

impl HyperParameterTuningJob {
    /// Materialise the transition the clock already owes: `InProgress` -> `Failed` for a
    /// job left to run, `Stopping` -> `Stopped` for one that was asked to stop.
    ///
    /// The one write a reporting operation may make: the transition was due at
    /// `settles_at` whether or not anybody described the job, so the observer makes no
    /// difference to the result. The end and last-modified times move to the instant the
    /// transition was due rather than to "now", so they are stable across later reads.
    ///
    /// A job left to run fails because this emulator launches no training job — see
    /// [`NO_TRAINING_RUNNER_FAILURE_REASON`]; a job asked to stop carries no failure
    /// reason, because stopping is what the caller asked for.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if now < self.settles_at {
            return false;
        }
        match self.status.as_str() {
            TUNING_STATUS_IN_PROGRESS => {
                self.status = TUNING_STATUS_FAILED.to_string();
                self.failure_reason = Some(NO_TRAINING_RUNNER_FAILURE_REASON.to_string());
            }
            TUNING_STATUS_STOPPING => self.status = TUNING_STATUS_STOPPED.to_string(),
            _ => return false,
        }
        self.end_time = Some(self.settles_at);
        self.last_modified_time = self.settles_at;
        true
    }

    /// Whether the job has reached a status from which nothing more happens on its own.
    ///
    /// `Deleting` and `DeleteFailed` belong to `DeleteHyperParameterTuningJob`, which this
    /// slice does not reach, so the terminal statuses are the three a job here can be in.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            TUNING_STATUS_FAILED | TUNING_STATUS_STOPPED | STATUS_COMPLETED
        )
    }

    /// Record that the caller asked this job to stop: it moves to `Stopping` now, and the
    /// clock owes it `Stopped`. The span is never zero, so the transient status is
    /// observable.
    pub fn request_stop(&mut self, now: SystemTime) {
        self.status = TUNING_STATUS_STOPPING.to_string();
        self.failure_reason = None;
        self.last_modified_time = now;
        self.settles_at = now + TUNING_JOB_STOPS_AFTER;
    }
}

/// A trial created by `CreateTrial`, as `DescribeTrial` replays it.
#[derive(Debug, Clone)]
pub struct Trial {
    pub name: String,
    pub arn: String,
    pub display_name: Option<String>,
    pub experiment_name: String,
    pub metadata_properties: Option<MetadataProperties>,
    pub tags: Vec<Tag>,
    /// Stored at creation rather than rebuilt per read, so every later read reports the
    /// identity that actually created the trial.
    pub created_by: UserContext,
    /// Who last modified the trial: the creating identity until a call modifies it, when
    /// `UpdateTrial` records the identity behind that call.
    pub last_modified_by: UserContext,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

/// A processing job created by `CreateProcessingJob`, as `DescribeProcessingJob`
/// replays it.
///
/// `ends_at` is the instant the clock owes the move out of the job's current
/// non-terminal status (`InProgress` from the create, `Stopping` from a stop); it is
/// stored, never slept on, and materialised by whichever reporting call first runs at or
/// after it (see [`ProcessingJob::settle_status`]).
#[derive(Debug, Clone)]
pub struct ProcessingJob {
    pub name: String,
    pub arn: String,
    pub role_arn: String,
    pub app_specification: AppSpecification,
    pub processing_inputs: Option<Vec<ProcessingInput>>,
    pub processing_output_config: Option<ProcessingOutputConfig>,
    /// The requested resources with the documented defaults applied and, when the
    /// request named `InstancePreferences`, the instance type the service selected
    /// from them.
    pub processing_resources: ProcessingResources,
    pub stopping_condition: Option<ProcessingStoppingCondition>,
    pub environment: Option<std::collections::HashMap<String, String>>,
    pub network_config: Option<NetworkConfig>,
    pub experiment_config: Option<ExperimentConfig>,
    pub tags: Vec<Tag>,
    pub status: String,
    pub failure_reason: Option<String>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub processing_end_time: Option<SystemTime>,
    pub ends_at: SystemTime,
}

impl ProcessingJob {
    /// Materialise the move out of a non-terminal status the clock already owes.
    ///
    /// This is the one write a reporting operation may make: the move was due at
    /// `ends_at` whether or not anybody described the job, so the observer makes no
    /// difference to the result. `LastModifiedTime` and `ProcessingEndTime` take the
    /// instant the move was due, not "now", so both are stable across later reads.
    ///
    /// A job left to run ends in [`PROCESSING_STATUS_FAILED`] with
    /// [`NO_CONTAINER_RUNTIME_REASON`]: nothing ran the job's container image, so no
    /// `Completed` status, `ProcessingStartTime` or `ExitMessage` is invented for it. A
    /// job the caller asked to stop ends in [`PROCESSING_STATUS_STOPPED`] with no failure
    /// reason instead: stopping is what the caller asked for, not a failure.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if now < self.ends_at {
            return false;
        }
        match self.status.as_str() {
            PROCESSING_STATUS_IN_PROGRESS => {
                self.status = PROCESSING_STATUS_FAILED.to_string();
                self.failure_reason = Some(NO_CONTAINER_RUNTIME_REASON.to_string());
            }
            PROCESSING_STATUS_STOPPING => {
                self.status = PROCESSING_STATUS_STOPPED.to_string();
            }
            // Already terminal: nothing is owed and nothing moves.
            _ => return false,
        }
        // The instant the run ended, which is when the move was due rather than "now", so
        // both are stable across later reads. No start time is recorded beside it: the
        // container never started.
        self.processing_end_time = Some(self.ends_at);
        self.last_modified_time = self.ends_at;
        true
    }

    /// Whether the job has reached a status nothing leaves on its own, i.e. one of
    /// `ProcessingJobStatus`'s terminal members.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            PROCESSING_STATUS_FAILED | PROCESSING_STATUS_STOPPED | STATUS_COMPLETED
        )
    }

    /// Whether a stop the caller already asked for is still in flight, i.e. the job sits
    /// in the transient `Stopping` status.
    pub fn is_stopping(&self) -> bool {
        self.status == PROCESSING_STATUS_STOPPING
    }

    /// Record that the caller asked this job to stop: it moves to the transient
    /// `Stopping` now, and the clock owes it `Stopped` at
    /// [`PROCESSING_JOB_STOPS_AFTER`]. The span is never zero, so the transient status is
    /// observable.
    pub fn request_stop(&mut self, now: SystemTime) {
        self.status = PROCESSING_STATUS_STOPPING.to_string();
        // The run is being abandoned on request, not failing, so nothing is reported
        // beside `Stopping` or the `Stopped` that follows it.
        self.failure_reason = None;
        self.processing_end_time = None;
        self.last_modified_time = now;
        self.ends_at = now + PROCESSING_JOB_STOPS_AFTER;
    }
}

// ---------------------------------------------------------------------------
// Inference Recommender: the recommendation jobs `CreateInferenceRecommendationsJob`
// accepts and `DescribeInferenceRecommendationsJob` replays.
// ---------------------------------------------------------------------------

/// See [`MODEL_QUOTA`]. `DeleteInferenceRecommendationsJob` is the release path AWS
/// documents for a recommendation job; it is still a scaffold stub in this emulator, so
/// today nothing releases a record and this bound is what keeps the store finite.
pub const INFERENCE_RECOMMENDATIONS_JOB_QUOTA: usize = 1_000;

/// The `RecommendationJobStatus` members this emulator moves a recommendation job
/// through (the modelled enum also carries `IN_PROGRESS`, `COMPLETED`, `STOPPING`,
/// `STOPPED`, `DELETING` and `DELETED`, which this slice does not reach). Named once
/// here so no handler compares a bare string literal.
pub const RECOMMENDATION_JOB_STATUS_PENDING: &str = "PENDING";
/// See [`RECOMMENDATION_JOB_STATUS_PENDING`].
pub const RECOMMENDATION_JOB_STATUS_FAILED: &str = "FAILED";

/// How long after `CreateInferenceRecommendationsJob` the clock owes the job's move out
/// of `PENDING` into its terminal status.
///
/// A span chosen by this emulator (AWS documents no duration for a recommendation run),
/// inside the 250–750 ms band the project fixes for a transition the clock owes. It is
/// never zero, so a caller that describes the job straight after creating it still
/// observes `PENDING`, and one that waits observes the terminal status, so a poller
/// terminates.
pub const RECOMMENDATION_JOB_SETTLES_AFTER: Duration = Duration::from_millis(400);

/// `RecommendationJobInputConfig$JobDurationInSeconds`'s documented ceiling:
/// "Specifies the maximum duration of the job, in seconds. The maximum value is 18,000
/// seconds." The model constrains the member only from below (`range 1..`), so the
/// ceiling is enforced in the handler.
pub const MAX_RECOMMENDATION_JOB_DURATION_IN_SECONDS: i32 = 18_000;

/// Why a recommendation job this emulator accepted ends in the modelled `FAILED` status.
///
/// An Inference Recommender job provisions endpoints, replays the job's sample payload
/// against the model's inference container under the requested traffic pattern and
/// reports the latency, throughput and cost it measured. This emulator runs no inference
/// container and generates no load, so nothing is measured: reporting `COMPLETED` with
/// `InferenceRecommendations` or `EndpointPerformances` would be metrics synthesised to
/// look computed, which the engine contract (`vendor/engine/src/lib.rs`) forbids. The run
/// therefore ends in the status the model carries for a job that produced no
/// recommendation, with this reason saying why. Everything around the run — accepting the
/// job, validating it, minting its ARN, replaying it, reporting its terminal status — is
/// real.
pub const NO_INFERENCE_BENCHMARK_REASON: &str =
    "This emulator runs no model inference container and generates no benchmark traffic, so \
     no load test was performed and no inference recommendation was measured.";

/// How the recommendation surfaces OVER a job state that the job itself is not there:
/// the entity as the service words it, the `JobName` the request carried, and
/// `was not found.`
///
/// A recommendation job is addressed by NAME only
/// (`RecommendationJobName`/`InferenceRecommendationsJobName`), so the identifier is the
/// one the caller sent — unquoted — and a consumer correlates the refusal with its own
/// input. Stated once here so the surfaces that report over a job cannot word the same
/// fact differently. The job's own READER words it differently, and says so:
/// [`inference_recommendations_job_not_found_for_account`].
pub fn inference_recommendations_job_not_found(job_name: &str) -> String {
    format!("Inference recommendations job {job_name} was not found.")
}

/// How `DescribeInferenceRecommendationsJob` states that the job it was asked to report
/// is not there: the same entity and the requested `JobName`, qualified by the ACCOUNT
/// the lookup was made in — a recommendation job is an account-scoped resource, and the
/// reader of the job itself names the account it looked in.
///
/// The account is the emulator's own canonical account id, the one every ARN this service
/// mints carries, so the sentence names the account the caller's request really resolved
/// against rather than one taken from the request.
pub fn inference_recommendations_job_not_found_for_account(job_name: &str) -> String {
    format!(
        "Inference recommendations job {job_name} for {} not found.",
        ::emulator_server::emulator_account_id()
    )
}

/// How `GetScalingConfigurationRecommendation` states that a request picked no
/// recommendation within the job it resolved: neither `RecommendationId` nor
/// `EndpointName` was given, so there is nothing to compute a scaling configuration from,
/// and the sentence names both members the caller may supply instead.
pub const NEITHER_RECOMMENDATION_ID_NOR_ENDPOINT_NAME: &str =
    "Neither RecommendationId or EndpointName were provided in the input. Please provide a \
     RecommendationId or EndpointName in the request.";

/// An Inference Recommender job created by `CreateInferenceRecommendationsJob`, as
/// `DescribeInferenceRecommendationsJob` replays it.
///
/// Every member mirrors what the create request carried, so a describe replays the
/// request rather than a value invented on the read path. `ends_at` is the instant the
/// clock owes the move out of `PENDING`; it is stored, never slept on, and materialised
/// by whichever reporting call first runs at or after it (see
/// [`InferenceRecommendationsJob::settle_status`]).
///
/// `output_config` and `tags` are stored because the create carried them, even though no
/// implemented reader surfaces them yet (`DescribeInferenceRecommendationsJob` reports
/// neither), the way [`CodeRepository`] stores its tags.
///
/// `recommendations` holds what the job MEASURED, which
/// `GetScalingConfigurationRecommendation` computes a scaling configuration from. It is
/// empty for a job this service accepted, because this emulator serves no model inference
/// container and generates no benchmark traffic (see
/// [`NO_INFERENCE_BENCHMARK_REASON`]); nothing synthesises an entry, and a reader with no
/// entry to answer from refuses rather than inventing one.
#[derive(Debug, Clone)]
pub struct InferenceRecommendationsJob {
    pub name: String,
    pub arn: String,
    pub job_type: String,
    pub role_arn: String,
    pub input_config: RecommendationJobInputConfig,
    pub job_description: Option<String>,
    pub stopping_conditions: Option<RecommendationJobStoppingConditions>,
    pub output_config: Option<RecommendationJobOutputConfig>,
    pub tags: Vec<Tag>,
    pub status: String,
    pub failure_reason: Option<String>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub completion_time: Option<SystemTime>,
    pub ends_at: SystemTime,
    pub recommendations: Vec<InferenceRecommendation>,
}

impl InferenceRecommendationsJob {
    /// A job record whose benchmark results this store was GIVEN rather than measured:
    /// the form a test (or a future reader of a seeded corpus) needs so
    /// `GetScalingConfigurationRecommendation` has recommendations to compute from.
    ///
    /// The members a create request would have carried are left at their empty values,
    /// and the job is terminal on arrival (`ends_at` is its `creation_time`), because no
    /// create call accepted it and so no transition is owed for it.
    pub fn seeded(
        name: String,
        arn: String,
        status: String,
        recommendations: Vec<InferenceRecommendation>,
        creation_time: SystemTime,
    ) -> Self {
        Self {
            name,
            arn,
            job_type: String::new(),
            role_arn: String::new(),
            input_config: RecommendationJobInputConfig::default(),
            job_description: None,
            stopping_conditions: None,
            output_config: None,
            tags: Vec::new(),
            status,
            failure_reason: None,
            creation_time,
            last_modified_time: creation_time,
            completion_time: Some(creation_time),
            ends_at: creation_time,
            recommendations,
        }
    }
    /// Materialise the move out of `PENDING` the clock already owes.
    ///
    /// This is the one write a reporting operation may make: the move was due at
    /// `ends_at` whether or not anybody described the job, so the observer makes no
    /// difference to the result. `LastModifiedTime` and `CompletionTime` take the instant
    /// the move was due, not "now", so both are stable across later reads.
    ///
    /// The terminal status is [`RECOMMENDATION_JOB_STATUS_FAILED`] with
    /// [`NO_INFERENCE_BENCHMARK_REASON`]: see that constant for why a job this emulator
    /// accepted cannot end `COMPLETED`.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == RECOMMENDATION_JOB_STATUS_PENDING && now >= self.ends_at {
            self.status = RECOMMENDATION_JOB_STATUS_FAILED.to_string();
            self.failure_reason = Some(NO_INFERENCE_BENCHMARK_REASON.to_string());
            self.completion_time = Some(self.ends_at);
            self.last_modified_time = self.ends_at;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A training job as `CreateTrainingJob` stores one, created at `created` and owed its
    /// terminal status [`TRAINING_JOB_ENDS_AFTER`] later.
    fn stored_job(created: SystemTime) -> TrainingJob {
        TrainingJob {
            name: "clock-probe".to_string(),
            arn: "arn:aws:sagemaker:us-east-1:000000000000:training-job/clock-probe".to_string(),
            status: TRAINING_STATUS_IN_PROGRESS.to_string(),
            secondary_status: Some(TRAINING_SECONDARY_STATUS_STARTING.to_string()),
            creation_time: created,
            last_modified_time: created,
            training_end_time: None,
            warm_pool_status: None,
            training_plan_arn: None,
            failure_reason: None,
            run: None,
            training_start_time: None,
            model_artifacts: None,
            final_metrics: Vec::new(),
            ends_at: created + TRAINING_JOB_ENDS_AFTER,
            tuning_job_arn: None,
            request: crate::ops::types::CreateTrainingJobInput::default(),
            tags: Vec::new(),
        }
    }

    /// RULE: the terminal status a reader materialises is the one the job's own RUN reported,
    /// and a job whose run has not reported is not settled at all however late the read is —
    /// so a run slower than the clock's span still settles into what it produced.
    ///
    /// The rows differ along WHEN the read happens and WHETHER the run has reported.
    #[test]
    fn a_job_is_settled_only_once_its_run_has_reported() {
        let created = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let past_due = created + TRAINING_JOB_ENDS_AFTER + Duration::from_millis(1);

        // Rows: the case, whether the run has reported, when the read happens, and the status
        // the read must leave behind.
        let rows: [(&str, Option<&str>, SystemTime, &str); 4] = [
            (
                "read before the span is over, run not reported",
                None,
                created,
                TRAINING_STATUS_IN_PROGRESS,
            ),
            (
                "read past the span, run still going",
                None,
                past_due,
                TRAINING_STATUS_IN_PROGRESS,
            ),
            (
                "read past the span, the run completed",
                Some(TRAINING_STATUS_COMPLETED),
                past_due,
                TRAINING_STATUS_COMPLETED,
            ),
            (
                "read past the span, the run failed",
                Some(TRAINING_STATUS_FAILED),
                past_due,
                TRAINING_STATUS_FAILED,
            ),
        ];

        for (case, reported, read_at, expected) in rows {
            let mut job = stored_job(created);
            if let Some(outcome) = reported {
                let outcome = if outcome == TRAINING_STATUS_COMPLETED {
                    crate::state::TrainingRunOutcome::Completed {
                        model_artifacts: "s3://bucket/clock-probe/output/model.tar.gz".to_string(),
                        final_metrics: Vec::new(),
                    }
                } else {
                    crate::state::TrainingRunOutcome::failed("ClientError: the probe's reason")
                };
                job.record_run(outcome, created);
            }
            let settled = job.settle_status(read_at);
            assert_eq!(
                (settled, job.status.as_str()),
                (expected != TRAINING_STATUS_IN_PROGRESS, expected),
                "case {case:?}: expected the status {expected}, got {:?}",
                job.status
            );
            // A second read changes nothing: the transition is the clock's.
            let again = job.settle_status(read_at);
            assert!(
                !again && job.status == expected,
                "case {case:?}: a second read must answer the same thing, got {:?}",
                job.status
            );
        }
    }

    /// RULE: the clock cannot owe a terminal status before the work is over — a run that
    /// reports after the span pushes the due instant out to the moment it reported, and the
    /// status it produced is then what a later read materialises.
    #[test]
    fn a_run_slower_than_the_span_pushes_the_due_instant_out() {
        let created = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let reported_at = created + TRAINING_JOB_ENDS_AFTER + Duration::from_secs(5);
        let mut job = stored_job(created);
        job.record_run(
            crate::state::TrainingRunOutcome::Completed {
                model_artifacts: "s3://bucket/clock-probe/output/model.tar.gz".to_string(),
                final_metrics: Vec::new(),
            },
            reported_at,
        );
        assert_eq!(
            job.ends_at, reported_at,
            "expected the due instant to be the one the run reported at, got {:?}",
            job.ends_at
        );
        assert!(
            !job.settle_status(reported_at - Duration::from_millis(1)),
            "a read before the run reported must settle nothing, got {:?}",
            job.status
        );
        assert!(
            job.settle_status(reported_at) && job.status == TRAINING_STATUS_COMPLETED,
            "expected the completed status the run produced, got {:?}",
            job.status
        );
        assert_eq!(
            job.training_time_in_seconds(),
            Some(6),
            "expected the whole seconds between the start and the end of the run, got {:?}",
            job.training_time_in_seconds()
        );
    }
}
