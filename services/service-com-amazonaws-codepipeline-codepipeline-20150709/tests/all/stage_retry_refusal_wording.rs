// SPDX-License-Identifier: CC0-1.0

//! What `RetryStageExecution` STATES when the stage a request names cannot be retried
//! now, end to end over the wire this service speaks.
//!
//! The rule under test: every refusal of a retry for a stage that cannot be re-run now
//! carries the one `StageNotRetryableException` statement that enumerates the three
//! conditions together — the stage holds no failed action, the execution has already been
//! superseded, or the pipeline definition changed since the actions failed — with the
//! stage name, the pipeline name and the execution id taken from the REQUEST, whichever
//! of those conditions actually refused it and whichever `retryMode` was asked for.
//!
//! The statement is part of the contract: a retry driver branches on the error code and
//! matches the text, and the pipeline name it carries is not anywhere else in the
//! response. So the table below spans the forms the two interpolated names take and every
//! way a retry is refused — a stage of the latest run that never failed, a stage of a run
//! a later one superseded (the failed one and the one that never ran), and an execution id
//! no run of the pipeline carries — and asserts the whole sentence each time. The contrast
//! row at the end retries the latest run's failed stage successfully, so the statement is
//! not a catch-all.
//!
//! The harness comes from the `execution_engine` and `terminal_state_refusals` suites, so
//! these pipelines run their real actions through the emulated S3 service and every
//! assertion goes through the router.

use serde_json::json;

use crate::execution_engine::{
    artifact_bucket, creation_run, declaration, deploy_stage, source_stage, wait_for_terminal,
    Emulator,
};
use crate::terminal_state_refusals::{refusal, REFUSAL_STATUS};

/// The bucket every pipeline here reads its source object from.
const SOURCE_BUCKET: &str = "retry-wording-source";

/// A key no object is ever written under, so the source action fails and the stage after
/// it never runs — the shape in which a stage holds no failed action to re-run.
const ABSENT_KEY: &str = "absent.zip";

/// The bucket the stage after the source would deliver to.
const DEPLOY_BUCKET: &str = "retry-wording-deploy";

/// The artifact the source action would produce.
const ARTIFACT: &str = "SourceArtifact";

/// The name the source stage of each declaration carries (the one stage that DOES fail).
const FAILED_STAGE: &str = "Source";

/// The longest `StageName` a request may carry: `smithy_query(query="operation",
/// operation="RetryStageExecution")` reports both `stageName` and `pipelineName` as
/// `len 1..100, pattern ^[A-Za-z0-9.@\-_]+$`.
const MAX_NAME_LENGTH: usize = 100;

/// A `PipelineExecutionId` matching the modelled pattern
/// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`) that no pipeline
/// minted, so the message's execution id can only have come from the request.
const UNMINTED_EXECUTION_ID: &str = "00000000-0000-4000-8000-00000000000a";

/// Both scopes `StageRetryMode` models (`smithy_query(query="operation",
/// operation="RetryStageExecution")` reports `ALL_ACTIONS | FAILED_ACTIONS`): the refusal
/// does not depend on which one a caller asked for.
const RETRY_MODES: [&str; 2] = ["FAILED_ACTIONS", "ALL_ACTIONS"];

/// The statement `StageNotRetryableException` carries, over the names a request gave.
fn expected_message(pipeline: &str, stage: &str, execution: &str) -> String {
    format!(
        "The stage named {stage} in the {pipeline} pipeline can't be retried now. Either it \
         contains no failed actions, the execution '{execution}' is already superseded, or \
         the pipeline definition has changed since the action(s) failed."
    )
}

#[tokio::test]
async fn every_retry_refusal_states_the_three_conditions_over_the_names_the_request_carried() {
    // (case, pipeline name, the name of the stage after the failing source) — the rows
    // differ along the FORM of the two names the statement interpolates: plain names, the
    // punctuation both patterns allow, and a stage name at the longest the model takes.
    let rows: Vec<(&str, &str, String)> = vec![
        ("plain names", "retry-wording", "Deploy".to_string()),
        (
            "names carrying the punctuation both patterns allow",
            "retry.wording@v1_2-x",
            "Deploy.Stage@v1_2-x".to_string(),
        ),
        (
            "a stage name at the longest the model allows",
            "retry-wording-max",
            "D".repeat(MAX_NAME_LENGTH),
        ),
    ];

    for (case, pipeline, stage) in rows {
        let emulator = Emulator::new();
        emulator.bucket(&artifact_bucket(pipeline)).await;
        emulator.bucket(SOURCE_BUCKET).await;
        emulator.bucket(DEPLOY_BUCKET).await;
        emulator
            .ok(
                "CreatePipeline",
                &declaration(
                    pipeline,
                    json!([
                        source_stage(SOURCE_BUCKET, ABSENT_KEY, ARTIFACT),
                        deploy_stage(&stage, ARTIFACT, DEPLOY_BUCKET, "out.zip", false),
                    ]),
                    Some("PARALLEL"),
                ),
            )
            .await;

        // Creating the pipeline runs it once; two more runs follow, each settled before
        // the next starts, so the newest run is the pipeline's latest and the two before
        // it have been left behind.
        let created = creation_run(&emulator, pipeline).await;
        wait_for_terminal(&emulator, pipeline, &created).await;
        let mut ids = Vec::new();
        for _ in 0..2 {
            let started = emulator
                .ok("StartPipelineExecution", &json!({"name": pipeline}))
                .await;
            let id = started["pipelineExecutionId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: the start reports an execution id"))
                .to_string();
            wait_for_terminal(&emulator, pipeline, &id).await;
            ids.push(id);
        }
        let superseded = ids[0].clone();
        let latest = ids[1].clone();

        // (reason, the stage the retry names, the execution it names) — every way a retry
        // is refused now, each reading the same statement.
        let refusals: Vec<(&str, &str, &str)> = vec![
            (
                "the latest run's stage that holds no failed action",
                stage.as_str(),
                latest.as_str(),
            ),
            (
                "a stage that never ran, in a run a later one left behind",
                stage.as_str(),
                superseded.as_str(),
            ),
            (
                "the FAILED stage of a run a later one left behind",
                FAILED_STAGE,
                superseded.as_str(),
            ),
            (
                "the FAILED stage of the run the pipeline's own creation started",
                FAILED_STAGE,
                created.as_str(),
            ),
            (
                "an execution id no run of this pipeline carries",
                stage.as_str(),
                UNMINTED_EXECUTION_ID,
            ),
        ];
        for (reason, named_stage, execution) in refusals {
            for mode in RETRY_MODES {
                let (status, code, message) = refusal(
                    &emulator,
                    "RetryStageExecution",
                    &json!({
                        "pipelineName": pipeline,
                        "stageName": named_stage,
                        "pipelineExecutionId": execution,
                        "retryMode": mode,
                    }),
                )
                .await;
                assert_eq!(
                    (status, code.as_str()),
                    (REFUSAL_STATUS, "StageNotRetryableException"),
                    "case {case:?} / {reason:?} / {mode}: expected \
                     {REFUSAL_STATUS} StageNotRetryableException, got {status} {code:?} \
                     ({message})"
                );
                assert_eq!(
                    message,
                    expected_message(pipeline, named_stage, execution),
                    "case {case:?} / {reason:?} / {mode}: unexpected refusal message, got \
                     {message:?}"
                );
            }
        }

        // The latest run's FAILED stage is retryable, so the statement above states a
        // refusal this operation actually decides rather than a blanket answer.
        let accepted = emulator
            .ok(
                "RetryStageExecution",
                &json!({
                    "pipelineName": pipeline,
                    "stageName": FAILED_STAGE,
                    "pipelineExecutionId": latest,
                    "retryMode": "FAILED_ACTIONS",
                }),
            )
            .await;
        assert_eq!(
            accepted["pipelineExecutionId"].as_str(),
            Some(latest.as_str()),
            "case {case:?}: a retry of the latest run's failed stage is accepted and reports \
             the execution it re-ran, got {accepted}"
        );
    }
}
