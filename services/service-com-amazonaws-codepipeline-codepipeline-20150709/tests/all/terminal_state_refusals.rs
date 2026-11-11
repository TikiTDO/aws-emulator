// SPDX-License-Identifier: CC0-1.0

//! What this service answers about a thing whose work has ENDED — a job a worker already
//! reported, and an execution that already reached a terminal status — end to end over
//! the wire the service speaks (`awsJson1_1`, `POST /` with `X-Amz-Target:
//! CodePipeline_20150709.<Operation>`).
//!
//! The rules under test:
//!
//! * a job that EXISTS but has already terminated still exists: a result reported for it
//!   is the state error `PutJobSuccessResult`/`PutJobFailureResult` model
//!   (`InvalidJobStateException`), never `JobNotFoundException`, which is what an id that
//!   was never issued is answered with — and which of that error's statements it carries
//!   is decided by the job's own status against the status the result reports;
//! * an execution with no action execution in progress cannot be stopped, and the
//!   refusal is ONE statement for both reasons it can be true (the pipeline holds no such
//!   execution, or the execution it holds has nothing in progress), naming the execution
//!   and the pipeline the REQUEST named and nothing about the execution's state.
//!
//! The rows span what those rules generalise over: each terminal `JobStatus` a worker can
//! put a job in against each result it can report for one, a job no worker has
//! acknowledged, and every terminal `PipelineExecutionStatus` an execution can be stopped
//! after — plus the ids that name nothing.
//!
//! The harness comes from `tests/all/execution_engine.rs`, so these pipelines run their
//! real actions through the emulated S3 service and every assertion goes through the
//! router.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::execution_engine::{
    artifact_bucket, declaration, deploy_stage, source_stage, status_of, wait_for_terminal,
    Emulator, WAIT,
};

/// The bucket the source object lives in.
const SOURCE_BUCKET: &str = "terminal-source";

/// The source object's key.
const SOURCE_KEY: &str = "source.zip";

/// The artifact the source action produces.
const ARTIFACT: &str = "SourceArtifact";

/// The bytes the source object carries.
const PAYLOAD: &[u8] = b"the source bytes";

/// The custom action type whose jobs these tests report results for.
const PROVIDER: &str = "TerminalWorker";

/// The refusal status every error these operations model carries.
pub(crate) const REFUSAL_STATUS: u16 = 400;

/// A `JobId` matching the modelled pattern that no job was ever issued under.
const UNISSUED_JOB_ID: &str = "00000000-0000-4000-8000-000000000000";

/// A `PipelineExecutionId` matching the modelled pattern that no pipeline minted.
const UNMINTED_EXECUTION_ID: &str = "00000000-0000-4000-8000-000000000001";

/// The error body of a refused call: its `__type` and its `message`.
///
/// `pub(crate)` with the harness below: the `refusal_statements` suite reads a refusal
/// the same way rather than growing a second copy.
pub(crate) async fn refusal(
    emulator: &Emulator,
    operation: &str,
    body: &Value,
) -> (u16, String, String) {
    let (status, response) = emulator.cp(operation, body).await;
    let code = response["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("{operation}: expected an error body, got {response}"))
        .to_string();
    let message = response["message"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    (status, code, message)
}

/// Create a pipeline whose `Build` stage is a custom action, start an execution of it,
/// and answer `(execution id, job id, nonce)` for the job that execution queues.
///
/// `pub(crate)` with the harness below: the `refusal_statements` suite reports results
/// for a claimable job through the same setup rather than growing a second copy.
pub(crate) async fn claimable_job(
    emulator: &Emulator,
    pipeline: &str,
) -> (String, String, String) {
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket(SOURCE_BUCKET).await;
    emulator.put_object(SOURCE_BUCKET, SOURCE_KEY, PAYLOAD).await;
    emulator
        .ok(
            "CreateCustomActionType",
            &json!({
                "category": "Build",
                "provider": PROVIDER,
                "version": "1",
                "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
                "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
            }),
        )
        .await;
    emulator
        .ok(
            "CreatePipeline",
            &declaration(
                pipeline,
                json!([
                    source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
                    json!({
                        "name": "Build",
                        "actions": [{
                            "name": "Work",
                            "actionTypeId": {
                                "category": "Build",
                                "owner": "Custom",
                                "provider": PROVIDER,
                                "version": "1",
                            },
                            "configuration": {"ProjectName": pipeline},
                            "inputArtifacts": [{"name": ARTIFACT}],
                        }],
                    }),
                ]),
                None,
            ),
        )
        .await;
    let execution = emulator
        .ok("StartPipelineExecution", &json!({"name": pipeline}))
        .await["pipelineExecutionId"]
        .as_str()
        .expect("the start reports an execution id")
        .to_string();

    let deadline = Instant::now() + WAIT;
    let mut job = Value::Null;
    while Instant::now() < deadline {
        let answer = emulator
            .ok(
                "PollForJobs",
                &json!({
                    "actionTypeId": {
                        "category": "Build",
                        "owner": "Custom",
                        "provider": PROVIDER,
                        "version": "1",
                    },
                    "maxBatchSize": 10,
                }),
            )
            .await;
        if let Some(found) = answer["jobs"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|candidate| {
                candidate["data"]["pipelineContext"]["pipelineExecutionId"] == json!(execution)
            })
        {
            job = found.clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let job_id = job["id"].as_str().unwrap_or_default().to_string();
    assert!(
        !job_id.is_empty(),
        "the execution's custom action must queue a job within {WAIT:?}, got {job}"
    );
    let nonce = job["nonce"]
        .as_str()
        .unwrap_or_else(|| panic!("a dispatched job carries a nonce, got {job}"))
        .to_string();
    (execution, job_id, nonce)
}

/// The request body that reports `result` for `job_id`, stating `detail` as what the
/// worker observed — the part of the result two reports of one status can differ in.
///
/// `pub(crate)` with the harness below.
pub(crate) fn result_body(result: &str, job_id: &str, detail: &str) -> Value {
    match result {
        "PutJobFailureResult" => json!({
            "jobId": job_id,
            "failureDetails": {"type": "JobFailed", "message": detail},
        }),
        _ => json!({
            "jobId": job_id,
            "executionDetails": {"summary": detail},
        }),
    }
}

/// RULE: a job the worker carried to a terminal status is still a job this account was
/// issued, so a later result that would CHANGE it is the state error — with the
/// statement its own status against the reported one makes true — while an id that was
/// never issued stays `JobNotFoundException`.
///
/// The rows differ along the status the job already holds, the result being reported for
/// it, and whether the id was ever issued at all. Every later report here states a
/// result the job does not hold; the report that RESTATES the one it does hold is the
/// `refusal_statements` suite's.
#[tokio::test]
async fn a_result_for_a_job_whose_work_ended_is_refused_as_a_state_error() {
    // (case, the result that terminated the job (None: leave it unacknowledged), the
    //  result reported afterwards, the error code, how the message reads)
    #[allow(clippy::type_complexity)]
    let rows: Vec<(&str, Option<&str>, &str, &str, fn(&str) -> String)> = vec![
        (
            "a success reported for a job that succeeded",
            Some("PutJobSuccessResult"),
            "PutJobSuccessResult",
            "InvalidJobStateException",
            |job_id| {
                format!(
                    "The status of job with id {job_id} is already set to the desired value. \
                     It cannot be updated with the same status, but different result or \
                     output variables."
                )
            },
        ),
        (
            "a failure reported for a job that succeeded",
            Some("PutJobSuccessResult"),
            "PutJobFailureResult",
            "InvalidJobStateException",
            |job_id| format!("Job with id {job_id} has already terminated"),
        ),
        (
            "a success reported for a job that failed",
            Some("PutJobFailureResult"),
            "PutJobSuccessResult",
            "InvalidJobStateException",
            |job_id| format!("Job with id {job_id} has already terminated"),
        ),
        (
            "a failure reported for a job that failed",
            Some("PutJobFailureResult"),
            "PutJobFailureResult",
            "InvalidJobStateException",
            |job_id| {
                format!(
                    "The status of job with id {job_id} is already set to the desired value. \
                     It cannot be updated with the same status, but different result or \
                     output variables."
                )
            },
        ),
        (
            "a success reported for a job no worker acknowledged",
            None,
            "PutJobSuccessResult",
            "InvalidJobStateException",
            |job_id| format!("Job with id {job_id} has not been acknowledged"),
        ),
        (
            "a failure reported for a job no worker acknowledged",
            None,
            "PutJobFailureResult",
            "InvalidJobStateException",
            |job_id| format!("Job with id {job_id} has not been acknowledged"),
        ),
    ];

    for (row, (case, terminated_by, reported, expected_code, expected_message)) in
        rows.into_iter().enumerate()
    {
        let emulator = Emulator::new();
        // The pipeline name is short because the artifact bucket is derived from it and
        // an S3 bucket name is at most 63 characters.
        let pipeline = format!("job-state-{row}");
        let (_, job_id, nonce) = claimable_job(&emulator, &pipeline).await;
        if let Some(result) = terminated_by {
            emulator
                .ok(
                    "AcknowledgeJob",
                    &json!({"jobId": job_id, "nonce": nonce}),
                )
                .await;
            emulator
                .ok(result, &result_body(result, &job_id, "reported"))
                .await;
        }

        let (status, code, message) = refusal(
            &emulator,
            reported,
            &result_body(reported, &job_id, "reported again, differently"),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, expected_code),
            "case {case:?}: unexpected refusal ({message})"
        );
        assert_eq!(
            message,
            expected_message(&job_id),
            "case {case:?}: unexpected refusal message"
        );

        // An id this account was never issued is the OTHER error, from the same call.
        let (status, code, _) = refusal(
            &emulator,
            reported,
            &result_body(reported, UNISSUED_JOB_ID, "reported"),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, "JobNotFoundException"),
            "case {case:?}: an id no job was issued under is not a state error"
        );
    }
}

/// RULE: a stop of an execution with nothing in progress is refused in ONE statement,
/// whichever of the two reasons made it true and whatever terminal status the execution
/// holds — and the refused stop leaves that status exactly as it was.
///
/// The rows differ along how the execution reached a terminal status (so they span the
/// statuses a stop can be refused after) and along ids that name nothing.
#[tokio::test]
async fn a_stop_of_an_execution_with_nothing_in_progress_is_refused_in_one_statement() {
    // (case, how the execution reaches its terminal status, the status it reaches)
    let rows: Vec<(&str, &str, &str)> = vec![
        ("an execution that succeeded", "succeed", "Succeeded"),
        ("an execution that failed", "fail", "Failed"),
        ("an execution that was stopped", "stop", "Stopped"),
    ];

    for (row, (case, how, expected_status)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let pipeline = format!("stop-state-{row}");
        let (execution, job_id, nonce) = claimable_job(&emulator, &pipeline).await;
        match how {
            "succeed" => {
                emulator
                    .ok(
                        "AcknowledgeJob",
                        &json!({"jobId": job_id, "nonce": nonce}),
                    )
                    .await;
                emulator
                    .ok("PutJobSuccessResult", &json!({"jobId": job_id}))
                    .await;
            }
            "fail" => {
                emulator
                    .ok(
                        "AcknowledgeJob",
                        &json!({"jobId": job_id, "nonce": nonce}),
                    )
                    .await;
                emulator
                    .ok(
                        "PutJobFailureResult",
                        &result_body("PutJobFailureResult", &job_id, "reported"),
                    )
                    .await;
            }
            _ => {
                emulator
                    .ok(
                        "StopPipelineExecution",
                        &json!({
                            "pipelineName": pipeline,
                            "pipelineExecutionId": execution,
                            "abandon": true,
                            "reason": "the first stop",
                        }),
                    )
                    .await;
            }
        }
        let settled = wait_for_terminal(&emulator, &pipeline, &execution).await;
        assert_eq!(
            settled["pipelineExecution"]["status"],
            json!(expected_status),
            "case {case:?}: expected the execution to reach {expected_status}, got {settled}"
        );

        // The one statement, for every row: the execution and the pipeline the REQUEST
        // named, and nothing about which of the two reasons it was.
        let expected_message = format!(
            "The pipeline execution with ID '{execution}' in '{pipeline}' can't be stopped \
             now. Either it does not exist in the pipeline, or the pipeline execution has no \
             action executions in progress."
        );
        for abandon in [false, true] {
            let (status, code, message) = refusal(
                &emulator,
                "StopPipelineExecution",
                &json!({
                    "pipelineName": pipeline,
                    "pipelineExecutionId": execution,
                    "abandon": abandon,
                    "reason": "rejected",
                }),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (REFUSAL_STATUS, "PipelineExecutionNotStoppableException"),
                "case {case:?} (abandon {abandon}): unexpected refusal ({message})"
            );
            assert_eq!(
                message, expected_message,
                "case {case:?} (abandon {abandon}): unexpected refusal message"
            );
        }
        assert_eq!(
            status_of(&emulator, &pipeline, &execution).await,
            expected_status,
            "case {case:?}: a refused stop must leave the status as it was"
        );

        // An id the pipeline never minted is the OTHER reason the same statement covers,
        // so it carries exactly that text with the id the request named.
        let (status, code, message) = refusal(
            &emulator,
            "StopPipelineExecution",
            &json!({
                "pipelineName": pipeline,
                "pipelineExecutionId": UNMINTED_EXECUTION_ID,
                "abandon": false,
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, "PipelineExecutionNotStoppableException"),
            "case {case:?}: an execution the pipeline does not hold is not stoppable"
        );
        assert_eq!(
            message,
            format!(
                "The pipeline execution with ID '{UNMINTED_EXECUTION_ID}' in '{pipeline}' \
                 can't be stopped now. Either it does not exist in the pipeline, or the \
                 pipeline execution has no action executions in progress."
            ),
            "case {case:?}: the same statement names the id the request named"
        );
    }
}

/// A pipeline with no custom action, so the stop rows above have a sibling that reaches
/// a terminal status with no job worker involved at all: the statement is the same.
#[tokio::test]
async fn the_same_statement_refuses_a_stop_of_a_plain_executions_terminal_status() {
    let emulator = Emulator::new();
    let pipeline = "stop-state-plain";
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket(SOURCE_BUCKET).await;
    emulator.bucket("terminal-deploy").await;
    emulator.put_object(SOURCE_BUCKET, SOURCE_KEY, PAYLOAD).await;
    emulator
        .ok(
            "CreatePipeline",
            &declaration(
                pipeline,
                json!([
                    source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
                    deploy_stage("Deploy", ARTIFACT, "terminal-deploy", "plain.zip", false),
                ]),
                None,
            ),
        )
        .await;
    let execution = emulator
        .ok("StartPipelineExecution", &json!({"name": pipeline}))
        .await["pipelineExecutionId"]
        .as_str()
        .expect("the start reports an execution id")
        .to_string();
    let settled = wait_for_terminal(&emulator, pipeline, &execution).await;
    assert_eq!(
        settled["pipelineExecution"]["status"],
        json!("Succeeded"),
        "the execution delivers and succeeds, got {settled}"
    );

    let (status, code, message) = refusal(
        &emulator,
        "StopPipelineExecution",
        &json!({
            "pipelineName": pipeline,
            "pipelineExecutionId": execution,
            "abandon": false,
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (REFUSAL_STATUS, "PipelineExecutionNotStoppableException"),
        "an execution that already succeeded is not stoppable ({message})"
    );
    assert_eq!(
        message,
        format!(
            "The pipeline execution with ID '{execution}' in '{pipeline}' can't be stopped \
             now. Either it does not exist in the pipeline, or the pipeline execution has no \
             action executions in progress."
        ),
        "the statement is the same one, whatever ran the execution"
    );
}
