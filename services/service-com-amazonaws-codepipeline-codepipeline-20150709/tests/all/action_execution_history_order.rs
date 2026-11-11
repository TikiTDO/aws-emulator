// SPDX-License-Identifier: CC0-1.0

//! The ORDER `ListActionExecutions` reports an execution history in, over the wire the
//! service speaks (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: CodePipeline_20150709.ListActionExecutions`).
//!
//! The rule under test: an execution history is reported MOST RECENT FIRST — by when
//! each action execution started, descending — so the first page of the listing is what
//! happened last. The rows drive real executions (a custom-action job worker reports the
//! source action's outcome, an approver answers each later stage), so the order asserted
//! is over records whose start times the execution plane itself set.
//!
//! The harness is `tests/all/pipeline_lifecycle.rs`'s, so every assertion goes through
//! the router the production emulator builds.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::pipeline_lifecycle::{creation_run, Emulator, ROLE_ARN};

/// How long a row waits for a step the clock owes (each is due `state::execution`'s
/// `ACTION_STEP` after the one before it).
const WAIT: Duration = Duration::from_secs(20);

/// The custom action type the source action of every row runs: `Source`, because a
/// pipeline's first stage carries source actions only, and `Custom`, because such an
/// action's outcome is a job worker's to report — which is what lets a row settle the
/// first stage at a moment of its own choosing, after the records of that stage exist.
fn source_action_type() -> Value {
    json!({"category": "Source", "owner": "Custom", "provider": "MyWorker", "version": "1"})
}

/// Register that action type: a poll is served only for one this account registered.
async fn register(emulator: &Emulator) {
    emulator
        .ok(
            "CreateCustomActionType",
            &json!({
                "category": "Source",
                "provider": "MyWorker",
                "version": "1",
                "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
                "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
            }),
        )
        .await;
}

/// One manual-approval action: it starts when its stage is entered and then waits for an
/// approver, so the row controls when the NEXT stage starts.
fn approval_action(name: &str) -> Value {
    json!({
        "name": name,
        "actionTypeId": {
            "category": "Approval",
            "owner": "AWS",
            "provider": "Manual",
            "version": "1",
        },
    })
}

/// A declaration whose first stage is a custom source action and whose later stages are
/// one manual approval each, named by `later_stages`.
fn declaration(name: &str, later_stages: &[(&str, &str)]) -> Value {
    let mut stages = vec![json!({
        "name": "Source",
        "actions": [{
            "name": "Src",
            "actionTypeId": source_action_type(),
            "configuration": {"S3Bucket": "my-bucket", "S3ObjectKey": "app.zip"},
        }],
    })];
    for (stage, action) in later_stages {
        stages.push(json!({"name": stage, "actions": [approval_action(action)]}));
    }
    json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": "my-bucket"},
        "stages": stages,
    })
}

/// The `(stageName, actionName)` of each record a listing reported, in the order it
/// reported them.
fn reported_actions(page: &Value) -> Vec<(String, String)> {
    details(page)
        .iter()
        .map(|detail| {
            (
                detail["stageName"].as_str().unwrap_or_default().to_string(),
                detail["actionName"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The `actionExecutionDetails` of a listing response.
fn details(page: &Value) -> Vec<Value> {
    page["actionExecutionDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing reports actionExecutionDetails, got {page}"))
        .clone()
}

/// A listing request for the action executions of ONE execution of `pipeline`.
fn only(pipeline: &str, execution_id: &str) -> Value {
    json!({"pipelineName": pipeline, "filter": {"pipelineExecutionId": execution_id}})
}

/// Poll until the source job is served, claim it, and report it successful — which is
/// what lets the execution leave the first stage.
async fn complete_the_source_job(emulator: &Emulator, case: &str) {
    let deadline = Instant::now() + WAIT;
    let (job_id, nonce) = loop {
        let polled = emulator
            .ok(
                "PollForJobs",
                &json!({"actionTypeId": source_action_type(), "maxBatchSize": 10}),
            )
            .await;
        if let Some(job) = polled["jobs"].as_array().and_then(|jobs| jobs.first()) {
            break (
                job["id"].as_str().unwrap_or_default().to_string(),
                job["nonce"].as_str().unwrap_or_default().to_string(),
            );
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: no source job was served within {WAIT:?}, last {polled}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    emulator
        .ok("AcknowledgeJob", &json!({"jobId": job_id, "nonce": nonce}))
        .await;
    emulator
        .ok("PutJobSuccessResult", &json!({"jobId": job_id}))
        .await;
}

/// Answer the approval of `stage`/`action` once the execution reaches it, so the stage
/// after it starts.
async fn approve(emulator: &Emulator, pipeline: &str, stage: &str, action: &str, case: &str) {
    let deadline = Instant::now() + WAIT;
    let token = loop {
        let state = emulator
            .ok("GetPipelineState", &json!({"name": pipeline}))
            .await;
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
            break token;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: {stage}/{action} never published an approval token within \
             {WAIT:?}, last {state}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    emulator
        .ok(
            "PutApprovalResult",
            &json!({
                "pipelineName": pipeline,
                "stageName": stage,
                "actionName": action,
                "token": token,
                "result": {"summary": "approved by the suite", "status": "Approved"},
            }),
        )
        .await;
}

/// Wait until the listing for `execution_id` reports `expected` records.
async fn wait_for_records(
    emulator: &Emulator,
    pipeline: &str,
    execution_id: &str,
    expected: usize,
    case: &str,
) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        let page = emulator
            .ok("ListActionExecutions", &only(pipeline, execution_id))
            .await;
        if details(&page).len() >= expected {
            return page;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the execution recorded fewer than {expected} action \
             executions within {WAIT:?}, last {page}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// RULE: the action-execution history is reported most recent first, so the record of
/// the action that started LAST is the first one on page one, and paging reports the
/// whole history in that same order.
#[tokio::test]
async fn the_action_execution_history_is_reported_most_recent_first() {
    // The rows differ along what the rule generalises over: how many stages the
    // execution runs, and so how many records the history carries and how far apart
    // their start times are.
    let rows: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("one stage after the source stage", vec![("Build", "Approve")]),
        (
            "two stages after the source stage",
            vec![("Build", "Approve"), ("Ship", "Ship-it")],
        ),
        (
            "three stages after the source stage",
            vec![("Build", "Approve"), ("Test", "Sign-off"), ("Ship", "Ship-it")],
        ),
    ];

    for (case, later_stages) in rows {
        let emulator = Emulator::new();
        register(&emulator).await;
        let pipeline = "history";
        emulator
            .ok(
                "CreatePipeline",
                &json!({"pipeline": declaration(pipeline, &later_stages)}),
            )
            .await;
        // Creating a pipeline runs it once, and that run is the one this row drives.
        let execution_id = creation_run(&emulator, pipeline).await;

        complete_the_source_job(&emulator, case).await;
        for (index, (stage, action)) in later_stages.iter().enumerate() {
            wait_for_records(&emulator, pipeline, &execution_id, index + 2, case).await;
            // The last stage's approval is left unanswered: its record exists either
            // way, and the row asserts about the history rather than the outcome.
            if index + 1 < later_stages.len() {
                approve(&emulator, pipeline, stage, action, case).await;
            }
        }

        let total = later_stages.len() + 1;
        let page = wait_for_records(&emulator, pipeline, &execution_id, total, case).await;

        // Most recent first: the source action started first, so its record is LAST.
        let mut expected: Vec<(String, String)> = vec![("Source".to_string(), "Src".to_string())];
        for (stage, action) in &later_stages {
            expected.push(((*stage).to_string(), (*action).to_string()));
        }
        expected.reverse();
        assert_eq!(
            reported_actions(&page),
            expected,
            "case {case:?}: expected the history most recent first, got {page}"
        );

        // The same order, read off the records' own start times: each one is at or
        // before the one reported before it.
        let starts: Vec<f64> = details(&page)
            .iter()
            .map(|detail| detail["startTime"].as_f64().unwrap_or_default())
            .collect();
        assert!(
            starts.windows(2).all(|pair| pair[0] >= pair[1]),
            "case {case:?}: expected non-increasing startTimes, got {starts:?}"
        );

        // What a reader asking for ONE record gets is the latest one, and paging from
        // there reports the whole history in the same order.
        let mut request = only(pipeline, &execution_id);
        request["maxResults"] = json!(1);
        let first = emulator.ok("ListActionExecutions", &request).await;
        assert_eq!(
            reported_actions(&first),
            expected[..1].to_vec(),
            "case {case:?}: page one of a one-record page is the latest record, got {first}"
        );

        let mut walked: Vec<(String, String)> = Vec::new();
        let mut next_token = first["nextToken"].as_str().map(str::to_string);
        walked.extend(reported_actions(&first));
        while let Some(token) = next_token {
            let mut request = only(pipeline, &execution_id);
            request["maxResults"] = json!(1);
            request["nextToken"] = json!(token);
            let page = emulator.ok("ListActionExecutions", &request).await;
            walked.extend(reported_actions(&page));
            next_token = page["nextToken"].as_str().map(str::to_string);
            assert!(
                walked.len() <= total,
                "case {case:?}: paging reported more records than the history holds: \
                 {walked:?}"
            );
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: paging reports the same records in the same order"
        );
    }
}
