// SPDX-License-Identifier: CC0-1.0

//! What a job worker REPORTS — `AcknowledgeJob`, `PutJobSuccessResult` and
//! `PutJobFailureResult` — over the wire the service actually speaks (`awsJson1_1`,
//! `POST /` with `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The rule under test: a `Custom`-owned action's work is performed by a job worker of
//! this account, so the outcome that worker reports IS the action's outcome. The worker
//! claims the job with the nonce its poll published, and from then on a success finishes
//! the action — unless the result carries a `continuationToken`, which identifies a
//! custom action still in progress, so the job goes back to waiting for the worker's next
//! report — while a failure fails the action; either way the engine rolls that outcome up
//! into the action's stage and into the execution. Only the worker holding a claimed job
//! may report for it, and the variables a success reports are what a later action of the
//! same execution resolves its configuration against.
//!
//! The jobs themselves — how an execution queues one and how a poll hands it out — are
//! the sibling suite's subject (`tests/all/custom_action_jobs.rs`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    MAX_OUTPUT_VARIABLES_BYTES, MAX_OUTPUT_VARIABLE_VALUE_BYTES,
};
use service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService;

/// The `X-Amz-Target` prefix OPERATIONS.md records for this service.
const TARGET: &str = "CodePipeline_20150709";

/// A `roleArn` satisfying `RoleArn`'s modelled pattern. IAM is not mounted here, so
/// `CreatePipeline`'s role check has no answer and the declaration is accepted.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/service-role/cp";

/// How long a test waits for a step the clock owes (each is due `ACTION_STEP`, 300 ms,
/// after the one before it).
const WAIT: Duration = Duration::from_secs(20);

/// The identity of the custom action type these pipelines run. Its category is `Source`
/// because the first stage of a pipeline must contain a source action.
const CATEGORY: &str = "Source";
/// See [`CATEGORY`].
const PROVIDER: &str = "MyWorker";

/// The status every refusal here carries.
const REFUSAL_STATUS: u16 = 400;

/// One emulator: the shared engine, the service mounted on it, and the router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/codepipeline/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";
        let request = http::Request::builder()
            .method("POST")
            .uri("https://codepipeline.us-east-1.amazonaws.com/")
            .header("host", "codepipeline.us-east-1.amazonaws.com")
            .header("authorization", authorization)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET}.{operation}"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or_else(|error| {
                panic!("{operation}: response body is not JSON ({error}): {body:?}")
            })
        };
        (status, parsed)
    }

    async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {response}"
        );
        response
    }

    /// The `(status, __type)` of a refusal.
    async fn refusal(&self, operation: &str, body: &Value) -> (u16, String) {
        let (status, response) = self.call(operation, body).await;
        let code = response
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{operation}: expected an error body, got {response}"))
            .to_string();
        (status, code)
    }
}

/// The action type the worker polls for.
fn action_type() -> Value {
    json!({
        "category": CATEGORY,
        "owner": "Custom",
        "provider": PROVIDER,
        "version": "1",
    })
}

/// Register that custom action type: a poll is served only for an action type this
/// account registered.
async fn register(emulator: &Emulator) {
    emulator
        .ok(
            "CreateCustomActionType",
            &json!({
                "category": CATEGORY,
                "provider": PROVIDER,
                "version": "1",
                "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
                "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
            }),
        )
        .await;
}

/// One `Custom`-owned action, with the `runOrder`, configuration and namespace it
/// declares.
fn worker_action(name: &str, run_order: i64, configuration: Value, namespace: Option<&str>) -> Value {
    let mut action = json!({
        "name": name,
        "actionTypeId": action_type(),
        "runOrder": run_order,
        "configuration": configuration,
    });
    if let Some(namespace) = namespace {
        action["namespace"] = json!(namespace);
    }
    action
}

/// Create a pipeline whose `Build` stage runs `actions`, and answer the id of the
/// execution its creation started.
///
/// Creating a pipeline RUNS it once — `TriggerType` carries `CreatePipeline` for that
/// run — so the pipeline's first execution is the one the create began, and that is the
/// execution whose job the worker in each case below reports for.
///
/// The `Gate` stage after it is the non-`Source` action a declaration has to carry
/// (`state::validate_declaration`): a manual approval does no work of its own, so the
/// worker's own outcome is still what settles the `Build` stage, and a case that waits
/// for the whole execution answers the approval through [`approve`].
async fn start(emulator: &Emulator, name: &str, actions: Value) -> String {
    emulator
        .ok(
            "CreatePipeline",
            &json!({"pipeline": {
                "name": name,
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": format!("{name}-artifacts")},
                "stages": [
                    {"name": "Build", "actions": actions},
                    {"name": GATE_STAGE, "actions": [{
                        "name": GATE_ACTION,
                        "actionTypeId": {
                            "category": "Approval",
                            "owner": "AWS",
                            "provider": "Manual",
                            "version": "1",
                        },
                    }]},
                ],
            }}),
        )
        .await;
    let listed = emulator
        .ok("ListPipelineExecutions", &json!({"pipelineName": name}))
        .await;
    listed["pipelineExecutionSummaries"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|summary| summary["trigger"]["triggerType"] == json!("CreatePipeline"))
        .and_then(|summary| summary["pipelineExecutionId"].as_str())
        .unwrap_or_else(|| panic!("creating {name} ran it once, got {listed}"))
        .to_string()
}

/// Poll until a job is served, answering its `(id, nonce)`.
async fn claimable_job(emulator: &Emulator, case: &str) -> (String, String) {
    let deadline = Instant::now() + WAIT;
    loop {
        let polled = emulator
            .ok(
                "PollForJobs",
                &json!({"actionTypeId": action_type(), "maxBatchSize": 10}),
            )
            .await;
        if let Some(job) = polled["jobs"].as_array().and_then(|jobs| jobs.first()) {
            return (
                job["id"].as_str().expect("a job carries its id").to_string(),
                job["nonce"]
                    .as_str()
                    .expect("a polled job carries the nonce that claims it")
                    .to_string(),
            );
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: no job was served within {WAIT:?}, last {polled}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Poll until a job is served and claim it, answering its id.
async fn claimed_job(emulator: &Emulator, case: &str) -> String {
    let (job_id, nonce) = claimable_job(emulator, case).await;
    let acknowledged = emulator
        .ok("AcknowledgeJob", &json!({"jobId": job_id, "nonce": nonce}))
        .await;
    assert_eq!(
        acknowledged["status"], json!("InProgress"),
        "case {case:?}: a claimed job is InProgress, got {acknowledged}"
    );
    job_id
}

/// The approval stage every fixture pipeline carries after its `Build` stage, and the
/// action in it.
const GATE_STAGE: &str = "Gate";
/// See [`GATE_STAGE`].
const GATE_ACTION: &str = "Approve";

/// Answer the approval the `Gate` stage publishes once the execution reaches it, so an
/// execution whose worker reported success can settle.
async fn approve(emulator: &Emulator, pipeline: &str, case: &str) {
    let deadline = Instant::now() + WAIT;
    let token = loop {
        let state = emulator
            .ok("GetPipelineState", &json!({"name": pipeline}))
            .await;
        let token = state["stageStates"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|stage| stage["stageName"] == json!(GATE_STAGE))
            .filter_map(|stage| stage["actionStates"].as_array())
            .flatten()
            .find(|action| action["actionName"] == json!(GATE_ACTION))
            .and_then(|action| action["latestExecution"]["token"].as_str())
            .map(str::to_string);
        if let Some(token) = token {
            break token;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the approval action never published a token within {WAIT:?}, \
             last {state}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    emulator
        .ok(
            "PutApprovalResult",
            &json!({
                "pipelineName": pipeline,
                "stageName": GATE_STAGE,
                "actionName": GATE_ACTION,
                "token": token,
                "result": {"summary": "approved by the suite", "status": "Approved"},
            }),
        )
        .await;
}

/// Wait until the execution reaches a terminal status, answering it.
async fn terminal_status(emulator: &Emulator, pipeline: &str, execution: &str, case: &str) -> String {
    let deadline = Instant::now() + WAIT;
    loop {
        let answer = emulator
            .ok(
                "GetPipelineExecution",
                &json!({"pipelineName": pipeline, "pipelineExecutionId": execution}),
            )
            .await;
        let status = answer["pipelineExecution"]["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if matches!(
            status.as_str(),
            "Succeeded" | "Failed" | "Stopped" | "Superseded" | "Cancelled"
        ) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the execution never reached a terminal status within {WAIT:?}, \
             last {answer}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// What `GetPipelineState` reports for one action of the `Build` stage.
async fn action_state(emulator: &Emulator, pipeline: &str, action: &str) -> Value {
    let state = emulator
        .ok("GetPipelineState", &json!({"name": pipeline}))
        .await;
    state["stageStates"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|reported| reported["stageName"] == json!("Build"))
        .and_then(|reported| {
            reported["actionStates"]
                .as_array()?
                .iter()
                .find(|reported| reported["actionName"] == json!(action))
                .cloned()
        })
        .unwrap_or_else(|| panic!("no action state for Build/{action} in {state}"))
}

/// RULE: the outcome a worker reports is the action's outcome, which the engine rolls up
/// into the stage and the execution.
///
/// The rows differ along what the worker reports.
#[tokio::test]
async fn the_outcome_a_worker_reports_is_the_actions_outcome() {
    // (case, the result the worker reports, the status the action reaches, the status the
    // execution reaches)
    let cases: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "a worker reporting success",
            "PutJobSuccessResult",
            json!({"executionDetails": {"summary": "the worker ran it"}}),
            "Succeeded",
            "Succeeded",
        ),
        (
            "a worker reporting success with nothing to say about it",
            "PutJobSuccessResult",
            json!({}),
            "Succeeded",
            "Succeeded",
        ),
        (
            "a worker reporting failure",
            "PutJobFailureResult",
            json!({"failureDetails": {"type": "JobFailed", "message": "the build broke"}}),
            "Failed",
            "Failed",
        ),
        (
            "a worker reporting a configuration failure",
            "PutJobFailureResult",
            json!({
                "failureDetails": {
                    "type": "ConfigurationError",
                    "message": "the action is misconfigured",
                },
            }),
            "Failed",
            "Failed",
        ),
    ];

    for (case, operation, mut result, action_status, execution_status) in cases {
        let emulator = Emulator::new();
        register(&emulator).await;
        let execution_id = start(
            &emulator,
            "demo",
            json!([worker_action("Work", 1, json!({"ProjectName": "demo"}), None)]),
        )
        .await;

        let job_id = claimed_job(&emulator, case).await;
        result["jobId"] = json!(job_id);
        emulator.ok(operation, &result).await;

        // A worker's success carries the execution into the approval stage, which only
        // a caller can answer; a failure settles the execution at the Build stage.
        if execution_status == "Succeeded" {
            approve(&emulator, "demo", case).await;
        }

        assert_eq!(
            terminal_status(&emulator, "demo", &execution_id, case).await,
            execution_status,
            "case {case:?}: the execution takes the outcome the worker reported"
        );
        let reported = action_state(&emulator, "demo", "Work").await;
        assert_eq!(
            reported["latestExecution"]["status"],
            json!(action_status),
            "case {case:?}: the action takes the outcome the worker reported, got {reported}"
        );
        let state = emulator
            .ok("GetPipelineState", &json!({"name": "demo"}))
            .await;
        assert_eq!(
            state["stageStates"][0]["latestExecution"]["status"],
            json!(action_status),
            "case {case:?}: the stage rolls the action's outcome up, got {state}"
        );
        if action_status == "Failed" {
            let code = result["failureDetails"]["type"].clone();
            assert_eq!(
                reported["latestExecution"]["errorDetails"]["code"],
                code,
                "case {case:?}: the action carries the FailureType the worker chose, got \
                 {reported}"
            );
        }

        // Whatever the outcome, the job it was reported for has TERMINATED: the job is
        // still one this account was issued, so a second result that would CHANGE it is
        // the state error — never `JobNotFoundException`, which is reserved for an id
        // that was never issued. (A second result RESTATING the recorded one asks for no
        // change at all, and is the `refusal_statements` suite's subject.)
        let (status, refusal) = emulator
            .refusal(
                "PutJobSuccessResult",
                &json!({
                    "jobId": job_id,
                    "executionDetails": {"summary": "a second, different report"},
                }),
            )
            .await;
        assert_eq!(
            (status, refusal.as_str()),
            (REFUSAL_STATUS, "InvalidJobStateException"),
            "case {case:?}: a terminated job takes no second result"
        );
    }
}

/// RULE: only the worker holding a CLAIMED job may report for it — a job nothing queued
/// is not found, a nonce that does not claim it is refused, and a job no worker has
/// claimed (or one already claimed by another worker) takes no result.
#[tokio::test]
async fn only_the_worker_holding_the_job_may_report_for_it() {
    let emulator = Emulator::new();
    register(&emulator).await;
    start(
        &emulator,
        "demo",
        json!([worker_action("Work", 1, json!({"ProjectName": "demo"}), None)]),
    )
    .await;
    let (job_id, nonce) = claimable_job(&emulator, "refusals").await;

    // A well-formed `JobId` (the uuid form `JobId` models) that nothing ever queued.
    let unknown = "00000000-0000-4000-8000-000000000000";
    // (case, operation, request, the error it is refused with)
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "an acknowledgement of a job nothing queued",
            "AcknowledgeJob",
            json!({"jobId": unknown, "nonce": nonce}),
            "JobNotFoundException",
        ),
        (
            "an acknowledgement carrying a nonce the poll never published",
            "AcknowledgeJob",
            json!({"jobId": job_id, "nonce": "not-the-nonce"}),
            "InvalidNonceException",
        ),
        (
            "a success result for a job nothing queued",
            "PutJobSuccessResult",
            json!({"jobId": unknown}),
            "JobNotFoundException",
        ),
        (
            "a failure result for a job nothing queued",
            "PutJobFailureResult",
            json!({
                "jobId": unknown,
                "failureDetails": {"type": "JobFailed", "message": "nope"},
            }),
            "JobNotFoundException",
        ),
        (
            "a success result for a job no worker has claimed",
            "PutJobSuccessResult",
            json!({"jobId": job_id}),
            "InvalidJobStateException",
        ),
        (
            "a failure result for a job no worker has claimed",
            "PutJobFailureResult",
            json!({
                "jobId": job_id,
                "failureDetails": {"type": "JobFailed", "message": "nope"},
            }),
            "InvalidJobStateException",
        ),
    ];
    for (case, operation, request, expected) in cases {
        let (status, code) = emulator.refusal(operation, &request).await;
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, expected),
            "case {case:?}: unexpected refusal"
        );
    }

    // The claim is exclusive: once it is granted, the nonce that granted it no longer
    // claims the job.
    emulator
        .ok("AcknowledgeJob", &json!({"jobId": job_id, "nonce": nonce}))
        .await;
    let (status, code) = emulator
        .refusal("AcknowledgeJob", &json!({"jobId": job_id, "nonce": nonce}))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (REFUSAL_STATUS, "InvalidNonceException"),
        "a job that is already being worked on is claimed by nobody else"
    );
}

/// RULE: a success result carrying a `continuationToken` does not finish the action — it
/// identifies "a custom action in progress", so the job goes back to waiting with that
/// token and the action stays `InProgress` until a result with no token arrives.
#[tokio::test]
async fn a_continuation_token_keeps_the_action_running() {
    let emulator = Emulator::new();
    register(&emulator).await;
    let execution_id = start(
        &emulator,
        "demo",
        json!([worker_action("Work", 1, json!({"ProjectName": "demo"}), None)]),
    )
    .await;

    let job_id = claimed_job(&emulator, "continuation").await;
    emulator
        .ok(
            "PutJobSuccessResult",
            &json!({"jobId": job_id, "continuationToken": "half-way"}),
        )
        .await;

    let running = action_state(&emulator, "demo", "Work").await;
    assert_eq!(
        running["latestExecution"]["status"],
        json!("InProgress"),
        "an action whose worker reported a continuation token has not finished, got \
         {running}"
    );
    assert_eq!(
        emulator
            .ok(
                "GetPipelineExecution",
                &json!({"pipelineName": "demo", "pipelineExecutionId": execution_id}),
            )
            .await["pipelineExecution"]["status"],
        json!("InProgress"),
        "nor has its execution"
    );

    // The job is waiting again, carrying the token the worker supplied, and a result
    // with no token then finishes the action.
    let (continued, nonce) = claimable_job(&emulator, "continuation").await;
    let details = emulator
        .ok("GetJobDetails", &json!({"jobId": continued}))
        .await;
    assert_eq!(
        details["jobDetails"]["data"]["continuationToken"],
        json!("half-way"),
        "the waiting job carries the token the worker supplied, got {details}"
    );
    emulator
        .ok("AcknowledgeJob", &json!({"jobId": continued, "nonce": nonce}))
        .await;
    emulator
        .ok(
            "PutJobSuccessResult",
            &json!({
                "jobId": continued,
                "executionDetails": {"summary": "the worker finished it"},
            }),
        )
        .await;
    approve(&emulator, "demo", "continuation").await;
    assert_eq!(
        terminal_status(&emulator, "demo", &execution_id, "continuation").await,
        "Succeeded",
        "a result with no continuation token finishes the action and the execution"
    );
}

/// RULE: the output variables a result reports are bounded — over the result as a whole
/// and per variable — and the ones that ARE reported are what a later action of the same
/// execution resolves its configuration against.
#[tokio::test]
async fn output_variables_are_bounded_and_reach_the_next_action() {
    let emulator = Emulator::new();
    register(&emulator).await;
    start(
        &emulator,
        "demo",
        json!([
            worker_action("Work", 1, json!({"ProjectName": "demo"}), Some("BuildVars")),
            worker_action(
                "Next",
                2,
                json!({"ProjectName": "deploy-#{BuildVars.ImageTag}"}),
                None,
            ),
        ]),
    )
    .await;
    let job_id = claimed_job(&emulator, "output variables").await;

    // (case, the variables reported) — each past one of the two bounds.
    let rows: Vec<(&str, Value)> = vec![
        (
            "more bytes than every variable together may carry",
            json!({
                "A": "x".repeat(MAX_OUTPUT_VARIABLE_VALUE_BYTES),
                "B": "x".repeat(MAX_OUTPUT_VARIABLE_VALUE_BYTES),
                "C": "x".repeat(MAX_OUTPUT_VARIABLES_BYTES),
            }),
        ),
        (
            "one variable longer than a configuration value can hold",
            json!({"Big": "x".repeat(MAX_OUTPUT_VARIABLE_VALUE_BYTES + 1)}),
        ),
    ];
    for (case, variables) in rows {
        let (status, code) = emulator
            .refusal(
                "PutJobSuccessResult",
                &json!({"jobId": job_id, "outputVariables": variables}),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, "OutputVariablesSizeExceededException"),
            "case {case:?}: output variables past the bound are refused"
        );
    }

    // A refusal leaves the job exactly as it was, so the worker can still report — and a
    // variable exactly AT the per-variable bound is taken, because the bound is a
    // maximum rather than a strict one.
    emulator
        .ok(
            "PutJobSuccessResult",
            &json!({
                "jobId": job_id,
                "outputVariables": {
                    "ImageTag": "v4",
                    "Edge": "y".repeat(MAX_OUTPUT_VARIABLE_VALUE_BYTES),
                },
            }),
        )
        .await;

    // The next action's job carries the configuration those variables resolve into.
    let deadline = Instant::now() + WAIT;
    loop {
        let (next, _) = claimable_job(&emulator, "output variables").await;
        let details = emulator
            .ok("GetJobDetails", &json!({"jobId": next}))
            .await;
        let data = &details["jobDetails"]["data"];
        if data["pipelineContext"]["action"]["name"] == json!("Next") {
            assert_eq!(
                data["actionConfiguration"]["configuration"]["ProjectName"],
                json!("deploy-v4"),
                "the later action's configuration resolves the earlier action's variable, \
                 got {details}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the later action's job was not served within {WAIT:?}, last {details}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
