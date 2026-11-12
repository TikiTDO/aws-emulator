// SPDX-License-Identifier: CC0-1.0

//! The batch-evaluation plane's state: the batch evaluation jobs this service holds.
//!
//! A batch evaluation job is MINTED by `StartBatchEvaluation` — the call that starts it
//! chooses the data source its agent session traces are pulled from, the evaluators and
//! insights to run and where the results are written, and this emulator mints the
//! `batchEvaluationId` and the ARN it is addressed by ([`mint_batch_evaluation_id`],
//! [`batch_evaluation_arn`]). `GetBatchEvaluation` reports one of them back, so a
//! `batchEvaluationId` this plane holds no job for is refused with the shared
//! not-found sentence ([`super::resource_not_found_message`]).
//!
//! A job's `status` is read off the CLOCK rather than stored:
//! `StartBatchEvaluation` answers 202 Accepted, so a job that was just started is still
//! `PENDING`, is `IN_PROGRESS` once [`BATCH_EVALUATION_TRANSITION_SPAN`] has passed and
//! has run to its end one span after that, with no call needed to make it so
//! ([`BatchEvaluationRecord::status_as_of`]). Nothing is slept for and nothing is
//! written by a read: the instants a transition falls due follow from the instant the
//! job was started.
//!
//! The plane has an explicit ceiling: `StartBatchEvaluation` is its growth site and a
//! caller chooses both the names it starts jobs under and the `clientToken` each insert
//! is recorded with, so it is bounded by [`MAX_BATCH_EVALUATIONS`] and
//! [`MAX_BATCH_EVALUATION_IDEMPOTENCY_RECORDS`], each evicting its least recently
//! started record once that maximum is passed.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    BatchEvaluationSummary, DataSourceConfig, EvaluationMetadata, Evaluator, Insight, OutputConfig,
};

use super::{
    mint_named_resource_id, resource_not_found_message, NotFoundResource, ARN_SERVICE,
    ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState,
};

/// The most batch evaluation jobs this emulator retains; the least recently started one
/// is evicted once the store passes it.
///
/// An EMULATOR ceiling, not an AWS quota: every `StartBatchEvaluation` appends a record,
/// so without one a caller looping on the start would grow the process heap without end.
pub const MAX_BATCH_EVALUATIONS: usize = 256;

/// The most `StartBatchEvaluation` `clientToken` records this emulator retains.
pub const MAX_BATCH_EVALUATION_IDEMPOTENCY_RECORDS: usize = 1_000;

/// How long each step of a batch evaluation job's lifecycle takes: `PENDING` until one
/// span has passed, `IN_PROGRESS` for the span after that.
///
/// `StartBatchEvaluation` answers 202 Accepted — the job is accepted for execution
/// rather than finished — and AWS publishes no duration for how long a batch evaluation
/// takes (it depends on how many sessions the data source yields), so this span is the
/// EMULATOR's own: long enough that a caller which starts a job and reads it back at once
/// still observes the transient status, short enough not to hold a test suite up.
pub const BATCH_EVALUATION_TRANSITION_SPAN: Duration = Duration::from_secs(3);

/// `BatchEvaluationStatus` for a job that has been accepted and has not started running
/// yet (source: smithy_query query="shape" shape="BatchEvaluationStatus", values PENDING
/// | IN_PROGRESS | COMPLETED | COMPLETED_WITH_ERRORS | FAILED | STOPPING | STOPPED |
/// DELETING).
pub const BATCH_EVALUATION_STATUS_PENDING: &str = "PENDING";

/// `BatchEvaluationStatus` for a job that is running, from the same enum.
pub const BATCH_EVALUATION_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// `BatchEvaluationStatus` for a job that has run to its end, from the same enum.
pub const BATCH_EVALUATION_STATUS_COMPLETED: &str = "COMPLETED";

/// One batch evaluation job: what `StartBatchEvaluation` persisted, which is what every
/// later read of the job reports.
#[derive(Debug, Clone)]
pub struct BatchEvaluationRecord {
    /// Minted by [`mint_batch_evaluation_id`]; never client-supplied.
    pub batch_evaluation_id: String,
    /// Minted by [`batch_evaluation_arn`] for the region the starting request was signed
    /// for.
    pub batch_evaluation_arn: String,
    pub name: String,
    pub description: Option<String>,
    /// Where the agent session traces to evaluate are pulled from.
    pub data_source_config: DataSourceConfig,
    pub evaluators: Option<Vec<Evaluator>>,
    pub insights: Option<Vec<Insight>>,
    /// The per-session ground truth and test-scenario identifiers the starting request
    /// supplied, kept with the job so what it was started with is what is held.
    pub evaluation_metadata: Option<EvaluationMetadata>,
    pub output_config: Option<OutputConfig>,
    pub tags: Option<HashMap<String, String>>,
    pub kms_key_arn: Option<String>,
    pub created_at: SystemTime,
    /// The value [`ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState::next_seq`]
    /// handed this job: its eviction order, and what keeps two jobs started in the same
    /// millisecond apart.
    pub seq: u64,
}

impl BatchEvaluationRecord {
    /// A job as the call that starts it leaves it: the configuration that call asked for,
    /// started at `created_at`.
    #[allow(clippy::too_many_arguments)]
    pub fn started(
        seq: u64,
        region: &str,
        name: String,
        description: Option<String>,
        data_source_config: DataSourceConfig,
        evaluators: Option<Vec<Evaluator>>,
        insights: Option<Vec<Insight>>,
        evaluation_metadata: Option<EvaluationMetadata>,
        output_config: Option<OutputConfig>,
        tags: Option<HashMap<String, String>>,
        kms_key_arn: Option<String>,
        created_at: SystemTime,
    ) -> Self {
        let batch_evaluation_id = mint_batch_evaluation_id(&name, created_at, seq);
        Self {
            batch_evaluation_arn: batch_evaluation_arn(region, &batch_evaluation_id),
            batch_evaluation_id,
            name,
            description,
            data_source_config,
            evaluators,
            insights,
            evaluation_metadata,
            output_config,
            tags,
            kms_key_arn,
            created_at,
            seq,
        }
    }

    /// The instant this job starts running, or `None` when that instant is not
    /// representable (so nothing can be past it).
    pub fn running_at(&self) -> Option<SystemTime> {
        self.created_at.checked_add(BATCH_EVALUATION_TRANSITION_SPAN)
    }

    /// The instant this job has run to its end, read the same way as
    /// [`Self::running_at`]: one more span after it started running.
    pub fn finished_at(&self) -> Option<SystemTime> {
        self.running_at()
            .and_then(|running_at| running_at.checked_add(BATCH_EVALUATION_TRANSITION_SPAN))
    }

    /// This job's `BatchEvaluationStatus` at `now`: `PENDING` until it starts running,
    /// `IN_PROGRESS` while it is, and `COMPLETED` once it has run to its end. Read off
    /// the stored start instant and the clock, so a report of the job states the
    /// transition that was already due without writing anything.
    pub fn status_as_of(&self, now: SystemTime) -> &'static str {
        match (self.running_at(), self.finished_at()) {
            (_, Some(finished_at)) if now >= finished_at => BATCH_EVALUATION_STATUS_COMPLETED,
            (Some(running_at), _) if now >= running_at => BATCH_EVALUATION_STATUS_IN_PROGRESS,
            _ => BATCH_EVALUATION_STATUS_PENDING,
        }
    }

    /// This job as the modelled `BatchEvaluationSummary`, as of `now`.
    ///
    /// The summary carries the same identity, configuration and clock-derived lifecycle a
    /// full report of the job does, so a listing and a read of one job cannot disagree;
    /// `evaluationResults` and `errorDetails` are left out for the same reason the full
    /// report leaves them out — no judge model runs here, so nothing has scored the job
    /// and nothing has failed.
    pub fn summary(&self, now: SystemTime) -> BatchEvaluationSummary {
        BatchEvaluationSummary {
            batch_evaluation_id: self.batch_evaluation_id.clone(),
            batch_evaluation_arn: self.batch_evaluation_arn.clone(),
            batch_evaluation_name: self.name.clone(),
            status: self.status_as_of(now).to_string(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            description: self.description.clone(),
            evaluators: self.evaluators.clone(),
            insights: self.insights.clone(),
            evaluation_results: None,
            error_details: None,
            kms_key_arn: self.kms_key_arn.clone(),
            updated_at: Some(::aws_smithy_types::DateTime::from(
                self.updated_at_as_of(now),
            )),
        }
    }

    /// The total order batch evaluation jobs are reported in: newest first, with the
    /// start sequence breaking a tie between two jobs started in the same millisecond, so
    /// one state always paginates the same way.
    pub fn order_key(&self) -> (SystemTime, u64) {
        (self.created_at, self.seq)
    }

    /// When this job last changed at `now`: the instant of the most recent lifecycle
    /// transition the clock has made due, which is its start instant while it is still
    /// `PENDING`.
    pub fn updated_at_as_of(&self, now: SystemTime) -> SystemTime {
        match (self.running_at(), self.finished_at()) {
            (_, Some(finished_at)) if now >= finished_at => finished_at,
            (Some(running_at), _) if now >= running_at => running_at,
            _ => self.created_at,
        }
    }
}

/// Mint a `batchEvaluationId` for a job of `name` started at `created_at`.
///
/// `BatchEvaluationId` publishes the pattern
/// `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`, so the id is the name the request
/// asked for followed by the ten-character discriminator
/// [`mint_named_resource_id`] builds — the shared form for every id of this service that
/// publishes that pattern.
pub fn mint_batch_evaluation_id(name: &str, created_at: SystemTime, seq: u64) -> String {
    mint_named_resource_id(name, created_at, seq)
}

/// The ARN a batch evaluation job is addressed by in `region`.
///
/// `BatchEvaluationArn` publishes no pattern of its own, only the ARN resource type it
/// references (`AWS::BedrockAgentCore::BatchEvaluate`, source: smithy_query
/// query="shape" shape="BatchEvaluationArn"); this service's ARNs spell a resource
/// segment as that type lowercased and hyphenated, which is also the path segment the
/// operation is bound to (`/evaluations/batch-evaluate`).
pub fn batch_evaluation_arn(region: &str, batch_evaluation_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("batch-evaluate/{batch_evaluation_id}"),
    )
}

impl ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState {
    /// The batch evaluation job this `StartBatchEvaluation` `clientToken` already
    /// started, or `None` when it has started none this store still holds.
    pub fn batch_evaluation_for_token(&self, client_token: &str) -> Option<&BatchEvaluationRecord> {
        self.batch_evaluation_idempotency
            .get(client_token)
            .and_then(|batch_evaluation_id| self.batch_evaluations.get(batch_evaluation_id))
    }

    /// Store a started batch evaluation job, recording `client_token` as the token that
    /// started it, and hold every bound this plane declares.
    pub fn insert_batch_evaluation(
        &mut self,
        record: BatchEvaluationRecord,
        client_token: Option<String>,
    ) {
        if let Some(token) = client_token {
            self.batch_evaluation_idempotency
                .insert(token, record.batch_evaluation_id.clone());
        }
        self.batch_evaluations
            .insert(record.batch_evaluation_id.clone(), record);

        while self.batch_evaluations.len() > MAX_BATCH_EVALUATIONS {
            // The least recently started job gives way first; the jobs a live workflow
            // just started are the ones that stay readable.
            let Some(oldest) = self
                .batch_evaluations
                .values()
                .min_by_key(|held| held.seq)
                .map(|held| held.batch_evaluation_id.clone())
            else {
                break;
            };
            self.batch_evaluations.remove(&oldest);
            // The tokens that pointed at it go with it: a token whose job is gone would
            // otherwise let a replayed start mint a SECOND job instead of answering with
            // the one it already started.
            self.batch_evaluation_idempotency
                .retain(|_, held| held != &oldest);
        }
        while self.batch_evaluation_idempotency.len() > MAX_BATCH_EVALUATION_IDEMPOTENCY_RECORDS {
            // A token whose job is already gone is of no use to a replay, so it gives way
            // before one that can still be answered.
            let Some(oldest) = self
                .batch_evaluation_idempotency
                .iter()
                .min_by_key(|(token, batch_evaluation_id)| {
                    (
                        self.batch_evaluations
                            .get(*batch_evaluation_id)
                            .map_or(0, |held| held.seq),
                        (*token).clone(),
                    )
                })
                .map(|(token, _)| token.clone())
            else {
                break;
            };
            self.batch_evaluation_idempotency.remove(&oldest);
        }
    }

    /// Every batch evaluation job this store holds, newest first
    /// ([`BatchEvaluationRecord::order_key`]) — the one order every page of a listing is
    /// cut from, so following a token reports each held job exactly once.
    pub fn batch_evaluations_newest_first(&self) -> Vec<&BatchEvaluationRecord> {
        let mut jobs: Vec<&BatchEvaluationRecord> = self.batch_evaluations.values().collect();
        jobs.sort_by_key(|job| std::cmp::Reverse(job.order_key()));
        jobs
    }

    /// The batch evaluation job a read addressed, or the sentence naming it as the
    /// missing resource — the SHARED not-found wording this service words every missed
    /// lookup with.
    pub fn resolve_batch_evaluation(
        &self,
        batch_evaluation_id: &str,
    ) -> Result<&BatchEvaluationRecord, String> {
        self.batch_evaluations
            .get(batch_evaluation_id)
            .ok_or_else(|| {
                resource_not_found_message(
                    NotFoundResource::BatchEvaluation,
                    batch_evaluation_id,
                )
            })
    }
}
