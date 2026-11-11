// SPDX-License-Identifier: CC0-1.0

//! WHAT this service's refusals STATE, end to end over the wire it speaks
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! A refusal's status, its error code AND its message are part of the contract: a client
//! branches on the code and matches the text, so each rule below is asserted as the
//! sentence the service states, with only the values the REQUEST carried interpolated
//! into it.
//!
//! The rules under test:
//!
//! * a result whose output variables go past the total this service accepts is refused
//!   `OutputVariablesSizeExceededException` stating that limit and, for every variable
//!   the request carried, the bytes it measured — so a worker learns WHICH variable
//!   spent the budget; a result exactly AT the limit is accepted, and the statement
//!   stays inside the length its message member is modelled with;
//! * `PutApprovalResult` tells its two `InvalidApprovalTokenException` cases apart by the
//!   sentence they carry: a token that is not the one in force names the action and the
//!   stage it was checked against, and an action that is not an approval action names the
//!   pipeline that does not include one — while a name it cannot resolve at all is the
//!   error for THAT name (`ActionNotFoundException`, `StageNotFoundException`), stated as
//!   what the enclosing resource does not include and what to do about it;
//! * a retry is refused `StageNotRetryableException` whenever the stage holds no failed
//!   action to re-run — a superseded execution and a stage that did not fail alike — with
//!   the one statement that enumerates the three conditions, over the names the request
//!   gave;
//! * a worker's report that RESTATES the result its job already holds is accepted (it
//!   asks for nothing the job has not recorded), while one reporting the same status with
//!   a different result is the state error, and one reporting the other status says the
//!   job has already terminated.
//!
//! The harness and the claimable job come from the `execution_engine` and
//! `terminal_state_refusals` suites, so these pipelines run their real actions through
//! the emulated S3 service and every assertion goes through the router.

use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    MAX_OUTPUT_VARIABLES_BYTES, MAX_OUTPUT_VARIABLE_VALUE_BYTES,
};

use crate::execution_engine::{
    artifact_bucket, declaration, deploy_stage, source_stage, wait_for_terminal, Emulator, WAIT,
};
use crate::terminal_state_refusals::{claimable_job, refusal, result_body, REFUSAL_STATUS};

/// The bucket the approval and retry pipelines read their source object from.
const SOURCE_BUCKET: &str = "statement-source";

/// That object's key.
const SOURCE_KEY: &str = "source.zip";

/// The artifact a source action produces.
const ARTIFACT: &str = "SourceArtifact";

/// The bytes the source object carries.
const PAYLOAD: &[u8] = b"the statement bytes";

/// A token matching `ApprovalToken`'s modelled pattern
/// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`) that no approval
/// published.
const UNPUBLISHED_TOKEN: &str = "00000000-0000-4000-8000-000000000000";

/// A second such token, so a row covers more than one value of the member.
const OTHER_UNPUBLISHED_TOKEN: &str = "ffffffff-ffff-4fff-bfff-ffffffffffff";

/// The longest `StageName` / `ActionName` a request may carry
/// (`smithy_query(query="operation", operation="PutApprovalResult")` reports each as
/// `len 1..100, pattern ^[A-Za-z0-9.@\-_]+$`).
const MAX_NAME_LENGTH: usize = 100;

/// Report `variables` for the job `job_id`.
fn success_with_variables(job_id: &str, variables: &Map<String, Value>) -> Value {
    json!({"jobId": job_id, "outputVariables": Value::Object(variables.clone())})
}

/// The output variables of one case: `(name, value length)` pairs, as the request
/// carries them.
fn variables_of(pairs: &[(&str, usize)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(name, length)| ((*name).to_string(), json!("x".repeat(*length))))
        .collect()
}

#[tokio::test]
async fn the_output_variable_size_refusal_states_the_limit_and_every_variable_it_measured() {
    // (case, the variables the result carries, the breakdown the refusal states) — the
    // rows differ along the count of variables, their names (and so the order the
    // breakdown puts them in, which is not the request's), and how far past the limit
    // they go. A variable's measured size is its name plus its value.
    #[allow(clippy::type_complexity)]
    let rows: Vec<(&str, Vec<(&str, usize)>, &str)> = vec![
        (
            "one variable far past the limit on its own",
            vec![("BigVar", 130_000)],
            "BigVar: 130006",
        ),
        (
            "one variable one byte past the limit",
            vec![("V", MAX_OUTPUT_VARIABLES_BYTES)],
            "V: 122881",
        ),
        (
            "three variables, stated in name order rather than the request's",
            vec![
                ("zeta", 2),
                ("Alpha", MAX_OUTPUT_VARIABLES_BYTES),
                ("mid", 3),
            ],
            "Alpha: 122885, mid: 6, zeta: 6",
        ),
    ];

    for (row, (case, pairs, breakdown)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        // The pipeline name is short because the artifact bucket is derived from it and
        // an S3 bucket name is at most 63 characters.
        let pipeline = format!("outvars-{row}");
        let (_, job_id, nonce) = claimable_job(&emulator, &pipeline).await;
        emulator
            .ok("AcknowledgeJob", &json!({"jobId": job_id, "nonce": nonce}))
            .await;

        let variables = variables_of(&pairs);
        let (status, code, message) = refusal(
            &emulator,
            "PutJobSuccessResult",
            &success_with_variables(&job_id, &variables),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, "OutputVariablesSizeExceededException"),
            "case {case:?}: output variables past the limit are refused ({message})"
        );
        assert_eq!(
            message,
            format!(
                "The total size of all output variables for this action exceeds the limit of \
                 {MAX_OUTPUT_VARIABLES_BYTES} bytes. Output variables (in bytes): [{breakdown}]"
            ),
            "case {case:?}: unexpected refusal message"
        );

        // The refusal recorded nothing: the job is still the worker's to report, and a
        // result inside the limit is taken.
        emulator
            .ok(
                "PutJobSuccessResult",
                &success_with_variables(&job_id, &variables_of(&[("Small", 8)])),
            )
            .await;
    }
}

#[tokio::test]
async fn output_variables_exactly_at_the_limit_are_accepted() {
    let emulator = Emulator::new();
    let pipeline = "outvars-bound";
    let (_, job_id, nonce) = claimable_job(&emulator, pipeline).await;
    emulator
        .ok("AcknowledgeJob", &json!({"jobId": job_id, "nonce": nonce}))
        .await;

    // Exactly the limit, spread over variables each inside the per-variable bound: 122
    // four-character names with a 996-byte value (1000 bytes each), and one more that
    // fills the remaining 880.
    let mut variables: Map<String, Value> = (0..122)
        .map(|index| {
            (
                format!("V{index:03}"),
                json!("x".repeat(MAX_OUTPUT_VARIABLE_VALUE_BYTES - 4)),
            )
        })
        .collect();
    variables.insert("W".to_string(), json!("x".repeat(879)));
    let total: usize = variables
        .iter()
        .map(|(name, value)| name.len() + value.as_str().unwrap_or_default().len())
        .sum();
    assert_eq!(
        total, MAX_OUTPUT_VARIABLES_BYTES,
        "the row must sit exactly ON the limit, got {total}"
    );

    emulator
        .ok(
            "PutJobSuccessResult",
            &success_with_variables(&job_id, &variables),
        )
        .await;
}

/// The most characters a message of this service may carry (`smithy_query(query="shape",
/// shape="Message")` reports `@length(min: 1, max: 5000)` on the member every error of
/// this service states its message in).
const MAX_MESSAGE_LENGTH: usize = 5_000;

#[tokio::test]
async fn the_output_variable_size_refusal_fits_the_modelled_message_length() {
    let emulator = Emulator::new();
    let pipeline = "outvars-many";
    let (_, job_id, nonce) = claimable_job(&emulator, pipeline).await;
    emulator
        .ok("AcknowledgeJob", &json!({"jobId": job_id, "nonce": nonce}))
        .await;

    // `OutputVariablesMap` is modelled with no bound on how many entries it carries, so
    // a result can go past the limit with thousands of small variables: 4000 of them,
    // 32 bytes each (a five-character name and a 27-character value), is 128000 bytes.
    let variables: Map<String, Value> = (0..4_000)
        .map(|index| (format!("N{index:04}"), json!("x".repeat(27))))
        .collect();
    let total: usize = variables
        .iter()
        .map(|(name, value)| name.len() + value.as_str().unwrap_or_default().len())
        .sum();
    assert!(
        total > MAX_OUTPUT_VARIABLES_BYTES,
        "the row must go past the limit, got {total}"
    );

    let (status, code, message) = refusal(
        &emulator,
        "PutJobSuccessResult",
        &success_with_variables(&job_id, &variables),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (REFUSAL_STATUS, "OutputVariablesSizeExceededException"),
        "output variables past the limit are refused whatever their count"
    );
    assert!(
        message.len() <= MAX_MESSAGE_LENGTH,
        "the statement must fit the modelled message length, got {} characters",
        message.len()
    );
    assert!(
        message.starts_with(&format!(
            "The total size of all output variables for this action exceeds the limit of \
             {MAX_OUTPUT_VARIABLES_BYTES} bytes. Output variables (in bytes): [N0000: 32"
        )) && message.ends_with(']'),
        "the statement is the same one, naming the variables in name order, got {message:?}"
    );
}

/// The declaration one approval row drives: an S3 source stage, and a stage whose single
/// action is a manual approval.
fn approval_declaration(pipeline: &str, stage: &str, action: &str) -> Value {
    declaration(
        pipeline,
        json!([
            source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
            json!({
                "name": stage,
                "actions": [{
                    "name": action,
                    "actionTypeId": {
                        "category": "Approval",
                        "owner": "AWS",
                        "provider": "Manual",
                        "version": "1",
                    },
                }],
            }),
        ]),
        None,
    )
}

/// Poll `GetPipelineState` until the named approval action publishes its token.
async fn approval_token(emulator: &Emulator, pipeline: &str, stage: &str, action: &str) -> String {
    let deadline = Instant::now() + WAIT;
    loop {
        let state = emulator.ok("GetPipelineState", &json!({"name": pipeline})).await;
        let token = state["stageStates"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|reported| reported["stageName"] == json!(stage))
            .filter_map(|reported| reported["actionStates"].as_array())
            .flatten()
            .find(|reported| reported["actionName"] == json!(action))
            .and_then(|reported| reported["latestExecution"]["token"].as_str())
            .map(str::to_string);
        if let Some(token) = token {
            return token;
        }
        assert!(
            Instant::now() < deadline,
            "{stage}/{action} published no approval token within {WAIT:?}; last state {state}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The body that answers the approval `stage`/`action` of `pipeline` with `token`.
fn approval_body(pipeline: &str, stage: &str, action: &str, token: &str) -> Value {
    json!({
        "pipelineName": pipeline,
        "stageName": stage,
        "actionName": action,
        "result": {"summary": "answered", "status": "Approved"},
        "token": token,
    })
}

#[tokio::test]
async fn the_approval_refusals_each_state_the_case_they_refuse() {
    // (case, pipeline, stage, action) — the rows differ along the alphabet and the length
    // of the two names the statements interpolate. The pipeline name stays short because
    // the artifact bucket is derived from it and an S3 bucket name is at most 63
    // characters.
    let rows: Vec<(&str, &str, String, String)> = vec![
        (
            "plain names",
            "appr-plain",
            "Approve".to_string(),
            "ApproveAction".to_string(),
        ),
        (
            "every punctuation the name patterns admit",
            "appr-punct",
            "Review_Gate.1".to_string(),
            "Sign.Off@2".to_string(),
        ),
        (
            "the longest stage and action names",
            "appr-long",
            "s".repeat(MAX_NAME_LENGTH),
            "a".repeat(MAX_NAME_LENGTH),
        ),
    ];

    for (case, pipeline, stage, action) in rows {
        let emulator = Emulator::new();
        emulator.bucket(&artifact_bucket(pipeline)).await;
        emulator.bucket(SOURCE_BUCKET).await;
        emulator.put_object(SOURCE_BUCKET, SOURCE_KEY, PAYLOAD).await;
        emulator
            .ok(
                "CreatePipeline",
                &approval_declaration(pipeline, &stage, &action),
            )
            .await;
        let token = approval_token(&emulator, pipeline, &stage, &action).await;

        // A token that is not the one in force: the statement names the token, the action
        // and the stage it was checked against.
        for presented in [UNPUBLISHED_TOKEN, OTHER_UNPUBLISHED_TOKEN] {
            let (status, code, message) = refusal(
                &emulator,
                "PutApprovalResult",
                &approval_body(pipeline, &stage, &action, presented),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (REFUSAL_STATUS, "InvalidApprovalTokenException"),
                "case {case:?}: a token that is not in force is refused ({message})"
            );
            assert_eq!(
                message,
                format!(
                    "Approval token \u{2018}{presented}\u{2019} either does not exist or is not \
                     the correct token for action {action} and stage {stage}."
                ),
                "case {case:?}: unexpected refusal message for a wrong token"
            );
        }

        // An action the stage does not declare is the OTHER error, naming the stage it
        // was looked for in.
        let absent = "NoSuchApprovalAction";
        let (status, code, message) = refusal(
            &emulator,
            "PutApprovalResult",
            &approval_body(pipeline, &stage, absent, &token),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, "ActionNotFoundException"),
            "case {case:?}: an action absent from the stage is not found ({message})"
        );
        assert_eq!(
            message,
            format!(
                "{stage} does not include an action named {absent}. Make sure you are using \
                 the correct name for the action, and then try again."
            ),
            "case {case:?}: unexpected refusal message for an absent action"
        );

        // A stage the pipeline does not declare is a third error again, stated the way
        // this plane states every name it cannot resolve.
        let absent_stage = "NoSuchStage";
        let (status, code, message) = refusal(
            &emulator,
            "PutApprovalResult",
            &approval_body(pipeline, absent_stage, &action, &token),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, "StageNotFoundException"),
            "case {case:?}: a stage the pipeline does not declare is not found ({message})"
        );
        assert_eq!(
            message,
            format!(
                "{pipeline} does not include a stage named {absent_stage}. Make sure you are \
                 using the correct name for the stage, and then try again."
            ),
            "case {case:?}: unexpected refusal message for an absent stage"
        );

        // An action that IS declared but is no approval action: the same code as a wrong
        // token, with the statement that tells the two cases apart — even presented with
        // the token the real approval published.
        let (status, code, message) = refusal(
            &emulator,
            "PutApprovalResult",
            &approval_body(pipeline, "Source", "Src", &token),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, "InvalidApprovalTokenException"),
            "case {case:?}: a non-approval action cannot be answered ({message})"
        );
        assert_eq!(
            message,
            format!("{pipeline} does not include an approval action named Src."),
            "case {case:?}: unexpected refusal message for a non-approval action"
        );

        // None of the refusals consumed the approval: the token in force still answers
        // it.
        emulator
            .ok(
                "PutApprovalResult",
                &approval_body(pipeline, &stage, &action, &token),
            )
            .await;
    }
}

#[tokio::test]
async fn a_retry_with_no_failed_action_to_re_run_states_the_three_conditions() {
    let emulator = Emulator::new();
    let pipeline = "retry-statement";
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket(SOURCE_BUCKET).await;
    emulator.bucket("statement-deploy").await;
    emulator.put_object(SOURCE_BUCKET, SOURCE_KEY, PAYLOAD).await;
    emulator
        .ok(
            "CreatePipeline",
            &declaration(
                pipeline,
                json!([
                    source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
                    deploy_stage("Deploy", ARTIFACT, "statement-deploy", "out.zip", false),
                ]),
                Some("PARALLEL"),
            ),
        )
        .await;

    let mut ids = Vec::new();
    for _ in 0..2 {
        let started = emulator
            .ok("StartPipelineExecution", &json!({"name": pipeline}))
            .await;
        let id = started["pipelineExecutionId"]
            .as_str()
            .expect("the start reports an execution id")
            .to_string();
        wait_for_terminal(&emulator, pipeline, &id).await;
        ids.push(id);
    }

    // (case, the execution the retry addresses, the stage it names) — a superseded
    // execution and a stage of the LATEST execution that did not fail: neither holds a
    // failed action, and both read the same statement.
    let rows: Vec<(&str, &str, &str)> = vec![
        ("an execution a later run superseded", &ids[0], "Source"),
        ("the latest execution's stage that did not fail", &ids[1], "Source"),
        ("the latest execution's other stage", &ids[1], "Deploy"),
    ];
    for (case, execution, stage) in rows {
        let (status, code, message) = refusal(
            &emulator,
            "RetryStageExecution",
            &json!({
                "pipelineName": pipeline,
                "stageName": stage,
                "pipelineExecutionId": execution,
                "retryMode": "FAILED_ACTIONS",
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, "StageNotRetryableException"),
            "case {case:?}: a stage with no failed action is not retryable ({message})"
        );
        assert_eq!(
            message,
            format!(
                "The stage named {stage} in the {pipeline} pipeline can't be retried now. \
                 Either it contains no failed actions, the execution '{execution}' is already \
                 superseded, or the pipeline definition has changed since the action(s) failed."
            ),
            "case {case:?}: unexpected refusal message"
        );
    }

    // A stage the declaration does not carry is the other error, so the classification
    // above is not a catch-all.
    let (status, code, message) = refusal(
        &emulator,
        "RetryStageExecution",
        &json!({
            "pipelineName": pipeline,
            "stageName": "Absent",
            "pipelineExecutionId": ids[1],
            "retryMode": "FAILED_ACTIONS",
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (REFUSAL_STATUS, "StageNotFoundException"),
        "a stage the pipeline does not declare is not found ({message})"
    );
}

#[tokio::test]
async fn a_report_restating_a_jobs_recorded_result_is_accepted_and_a_differing_one_is_not() {
    // (case, the result that terminates the job, the result reporting the OTHER status)
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            "a job its worker reported successful",
            "PutJobSuccessResult",
            "PutJobFailureResult",
        ),
        (
            "a job its worker reported failed",
            "PutJobFailureResult",
            "PutJobSuccessResult",
        ),
    ];

    for (row, (case, reported, other)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let pipeline = format!("restate-{row}");
        let (_, job_id, nonce) = claimable_job(&emulator, &pipeline).await;
        emulator
            .ok("AcknowledgeJob", &json!({"jobId": job_id, "nonce": nonce}))
            .await;
        emulator
            .ok(reported, &result_body(reported, &job_id, "reported"))
            .await;

        // The same report again — the same status, the same result — asks for nothing the
        // job has not recorded, so it is accepted, and stays accepted however often the
        // worker repeats it.
        for _ in 0..2 {
            emulator
                .ok(reported, &result_body(reported, &job_id, "reported"))
                .await;
        }

        // The same status with a DIFFERENT result is the state error that says so.
        let (status, code, message) = refusal(
            &emulator,
            reported,
            &result_body(reported, &job_id, "a different observation"),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, "InvalidJobStateException"),
            "case {case:?}: the same status with another result is refused ({message})"
        );
        assert_eq!(
            message,
            format!(
                "The status of job with id {job_id} is already set to the desired value. It \
                 cannot be updated with the same status, but different result or output \
                 variables."
            ),
            "case {case:?}: unexpected refusal message for a changed result"
        );

        // And the other status is refused as the terminated job it is: accepting the
        // restatement moved nothing.
        let (status, code, message) =
            refusal(&emulator, other, &result_body(other, &job_id, "reported")).await;
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, "InvalidJobStateException"),
            "case {case:?}: the other status is refused ({message})"
        );
        assert_eq!(
            message,
            format!("Job with id {job_id} has already terminated"),
            "case {case:?}: unexpected refusal message for the other status"
        );
    }
}
