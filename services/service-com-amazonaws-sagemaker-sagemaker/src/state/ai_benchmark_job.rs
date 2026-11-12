// SPDX-License-Identifier: CC0-1.0

//! The AI-benchmark-job slice of this service's state: the [`AIBenchmarkJob`] record,
//! its bound, its statuses and the clock-owed end every reader materialises.

use super::*;

/// The ARN resource kind an AI benchmark job carries, i.e. the `<kind>` in
/// [`resource_arn`]. Fixed by `AIBenchmarkJobArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:ai-benchmark-job/...`), and named
/// once here so the create and the describe mint and match the same ARN.
pub const AI_BENCHMARK_JOB_KIND: &str = "ai-benchmark-job";

/// See [`MODEL_QUOTA`]. Bounds the AI benchmark job store the same way, and is what
/// `CreateAIBenchmarkJob` answers `ResourceLimitExceeded` past.
pub const AI_BENCHMARK_JOB_QUOTA: usize = 1_000;

/// Why a reader could not resolve an AI benchmark job: the family's
/// [`ai_entity_not_found`], quoting the name the request presented. Named once here so
/// every operation that resolves a benchmark-job reference words the same fact the same
/// way.
pub fn ai_benchmark_job_not_found(name: &str) -> String {
    ai_entity_not_found(AI_BENCHMARK_JOB_ENTITY, name)
}

/// How SageMaker states a `BenchmarkTarget` naming an inference endpoint this account does
/// not hold, quoting the identifier the request carried.
///
/// `CreateAIBenchmarkJob`'s target is "the target endpoint to benchmark" — a reference to a
/// SageMaker endpoint resolved before the job exists — so a reference nothing answers backs
/// no benchmark and the create is refused for it rather than accepted into a job that could
/// never drive traffic anywhere. The benchmarked resource is an ENDPOINT rather than one of
/// the AI-benchmarking family's own entities, so the sentence names it the way SageMaker's
/// endpoint vocabulary does rather than through [`ai_entity_not_found`].
pub fn benchmark_endpoint_not_found(identifier: &str) -> String {
    format!("SageMaker endpoint '{identifier}' does not exist")
}

/// Why a delete of a benchmark job whose run has not finished is refused: the state it is in
/// and the states a job can be deleted in.
///
/// `DeleteAIBenchmarkJob` releases a job, and a job still running is holding the work the
/// caller asked for: the delete is refused and the job survives it, rather than destroying
/// work in flight. The sentence names the entity, quotes the `AIBenchmarkJobStatus` the job
/// carries at the moment of the delete (read from state), and then lists the statuses nothing
/// leaves on its own — the three [`AIBenchmarkJob::is_terminal`] admits — so a caller knows
/// what to wait for. The operation models only `ResourceNotFound`, and the job IS found, so
/// the refusal is a 400 `ValidationException` carrying this sentence.
pub fn ai_benchmark_job_not_deletable(status: &str) -> String {
    format!(
        "{AI_BENCHMARK_JOB_ENTITY} is in state '{status}' and cannot be deleted. Only \
         {STATUS_COMPLETED}, {AI_BENCHMARK_STATUS_FAILED}, or {AI_BENCHMARK_STATUS_STOPPED} \
         jobs can be deleted."
    )
}

/// `AIBenchmarkJobStatus`'s members this emulator moves a benchmark job through
/// (`smithy_query(query="shape",shape="AIBenchmarkJobStatus")` answers `InProgress`,
/// `Completed`, `Failed`, `Stopping` and `Stopped`; `Completed` is not reached, because
/// nothing here benchmarks a target). Named once here so no handler compares a bare string
/// literal.
pub const AI_BENCHMARK_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`AI_BENCHMARK_STATUS_IN_PROGRESS`].
pub const AI_BENCHMARK_STATUS_FAILED: &str = "Failed";
/// See [`AI_BENCHMARK_STATUS_IN_PROGRESS`]: the transient status `StopAIBenchmarkJob` moves
/// a running job into.
pub const AI_BENCHMARK_STATUS_STOPPING: &str = "Stopping";
/// See [`AI_BENCHMARK_STATUS_IN_PROGRESS`]: the terminal status the clock owes a job the
/// caller asked to stop.
pub const AI_BENCHMARK_STATUS_STOPPED: &str = "Stopped";

/// Why a stop of a job that is no longer running is refused, naming the job and the status it
/// had already reached.
///
/// `StopAIBenchmarkJob` stops "a running AI benchmark job"
/// (`smithy_query(query="doc",shape="StopAIBenchmarkJob")`), so a job that has already
/// reached a status nothing leaves on its own has no run to stop. The operation models only
/// `ResourceNotFound`, and the job IS found, so the refusal is a 400 `ValidationException`
/// carrying this sentence — the form this crate's other stops use (see
/// [`ai_recommendation_job_not_running`]).
pub fn ai_benchmark_job_not_running(name: &str, status: &str) -> String {
    format!(
        "The AI benchmark job \"{name}\" is {status} and is not running, so there is \
         nothing to stop."
    )
}

/// `FailureReason` of a benchmark job the caller asked to stop.
///
/// The stop is not a fault of the job, but the service still records WHY the run ended
/// without a result, and it is the same sentence for every job a caller stops, so it is fixed
/// text with nothing of the request in it. Written where the stop is recorded
/// ([`AIBenchmarkJob::request_stop`]), so the reason is reported while the job is stopping
/// and after it has stopped.
pub const AI_BENCHMARK_STOPPED_BY_USER_REASON: &str = "Job stopped by user request";

/// How long after `CreateAIBenchmarkJob` the clock owes the job's move out of
/// `InProgress` into its terminal status.
///
/// A span chosen by this emulator (AWS documents no duration for a benchmark run),
/// inside the 250–750 ms band the project fixes for a transition the clock owes. It is
/// never zero, so a caller that describes the job straight after creating it still
/// observes `InProgress`, and one that waits observes the terminal status, so a waiter
/// terminates.
pub const AI_BENCHMARK_JOB_ENDS_AFTER: Duration = Duration::from_millis(400);

/// How long after `StopAIBenchmarkJob` the clock owes the job's `Stopping` -> `Stopped` move.
///
/// Chosen on the same terms as [`AI_BENCHMARK_JOB_ENDS_AFTER`] (AWS documents no duration for
/// the stop either) and inside the same band: never zero, so a caller that describes the job
/// straight after asking it to stop still observes the transient `Stopping`, and one that
/// waits observes `Stopped`, so a waiter terminates.
pub const AI_BENCHMARK_JOB_STOPS_AFTER: Duration = Duration::from_millis(300);

/// `FailureReason` of a benchmark job whose run nothing could perform here.
///
/// A benchmark job drives real traffic at a live inference endpoint and measures its
/// latency, throughput and cost. This emulator serves no inference data plane and runs
/// no benchmark tool, and the engine contract (`vendor/engine/src/lib.rs`) forbids
/// reporting a status, duration or measurement for work nothing ran — so the run the
/// caller asked for ends in the modelled `Failed` state with this reason, and no
/// `StartTime`, `Completed` status or benchmark result is invented. Everything around
/// the run — accepting the job, resolving its workload configuration, minting its ARN,
/// replaying it, reporting its terminal status — is real.
pub const NO_BENCHMARK_RUNTIME_REASON: &str =
    "This emulator serves no inference data plane and runs no benchmark tool, so the \
     benchmark job's run was never started";

/// An AI benchmark job created by `CreateAIBenchmarkJob`, as `DescribeAIBenchmarkJob`
/// replays it.
///
/// `ends_at` is the instant the clock owes the move out of `InProgress`; it is stored,
/// never slept on, and materialised by whichever reporting call first runs at or after
/// it (see [`AIBenchmarkJob::settle_status`]).
#[derive(Debug, Clone)]
pub struct AIBenchmarkJob {
    pub name: String,
    pub arn: String,
    /// The `AIWorkloadConfigIdentifier` the request presented, kept VERBATIM (a bare
    /// name or the configuration's ARN) so the read replays what the caller sent rather
    /// than a form it did not use.
    pub workload_config_identifier: String,
    pub benchmark_target: crate::ops::types::AIBenchmarkTarget,
    /// What the request's `OutputConfig` carried. `DescribeAIBenchmarkJob` reports it as
    /// an `AIBenchmarkOutputResult`, which adds the log streams of a run — and nothing
    /// ran here, so that member stays absent.
    pub output_config: crate::ops::types::AIBenchmarkOutputConfig,
    pub network_config: Option<crate::ops::types::AIBenchmarkNetworkConfig>,
    pub role_arn: String,
    pub tags: Vec<Tag>,
    pub status: String,
    pub failure_reason: Option<String>,
    pub creation_time: SystemTime,
    pub end_time: Option<SystemTime>,
    pub ends_at: SystemTime,
}

impl AIBenchmarkJob {
    /// Materialise the move out of a non-terminal status the clock already owes.
    ///
    /// This is the one write a reporting operation may make: the move was due at
    /// `ends_at` whether or not anybody described the job, so the observer makes no
    /// difference to the result. `EndTime` takes the instant the move was due, not
    /// "now", so it is stable across later reads.
    ///
    /// A job left to run ends in [`AI_BENCHMARK_STATUS_FAILED`] with
    /// [`NO_BENCHMARK_RUNTIME_REASON`]: nothing benchmarked the target, so no `Completed`
    /// status and no `StartTime` is invented for it. A job the caller asked to stop ends in
    /// [`AI_BENCHMARK_STATUS_STOPPED`], keeping the reason the stop recorded
    /// ([`AI_BENCHMARK_STOPPED_BY_USER_REASON`]).
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if now < self.ends_at {
            return false;
        }
        match self.status.as_str() {
            AI_BENCHMARK_STATUS_IN_PROGRESS => {
                self.status = AI_BENCHMARK_STATUS_FAILED.to_string();
                self.failure_reason = Some(NO_BENCHMARK_RUNTIME_REASON.to_string());
            }
            AI_BENCHMARK_STATUS_STOPPING => {
                self.status = AI_BENCHMARK_STATUS_STOPPED.to_string();
            }
            // Already terminal: nothing is owed and nothing moves.
            _ => return false,
        }
        // The instant the run ended, which is when the move was due rather than "now", so it
        // is stable across later reads. No start time is recorded beside it: the run never
        // started.
        self.end_time = Some(self.ends_at);
        true
    }

    /// Whether the job has reached a status nothing leaves on its own, i.e. one of
    /// `AIBenchmarkJobStatus`'s terminal members (`Completed`, `Failed` and `Stopped`).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            AI_BENCHMARK_STATUS_FAILED | AI_BENCHMARK_STATUS_STOPPED | super::STATUS_COMPLETED
        )
    }

    /// Whether a stop the caller already asked for is still in flight, i.e. the job sits in
    /// the transient `Stopping` status.
    pub fn is_stopping(&self) -> bool {
        self.status == AI_BENCHMARK_STATUS_STOPPING
    }

    /// Record that the caller asked this job to stop: it moves to the transient `Stopping`
    /// now, and the clock owes it `Stopped` at [`AI_BENCHMARK_JOB_STOPS_AFTER`]. The span is
    /// never zero, so the transient status is observable.
    pub fn request_stop(&mut self, now: SystemTime) {
        self.status = AI_BENCHMARK_STATUS_STOPPING.to_string();
        // The run is being abandoned on request, so the reason it ended without a result is
        // the stop the caller asked for — replacing whatever reason a previous status carried.
        self.failure_reason = Some(AI_BENCHMARK_STOPPED_BY_USER_REASON.to_string());
        self.end_time = None;
        self.ends_at = now + AI_BENCHMARK_JOB_STOPS_AFTER;
    }
}

/// The endpoint identifier an `AIBenchmarkTarget` addresses: "the name or Amazon Resource
/// Name (ARN) of the SageMaker endpoint to benchmark", as the request carried it.
///
/// A single reading of the union, so the create that resolves the target and any later
/// operation that reports it cannot disagree about which member carries the reference.
pub fn benchmark_target_endpoint(target: &crate::ops::types::AIBenchmarkTarget) -> &str {
    match target {
        crate::ops::types::AIBenchmarkTarget::Endpoint(endpoint) => endpoint.identifier.as_str(),
    }
}
