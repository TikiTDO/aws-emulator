// SPDX-License-Identifier: CC0-1.0

//! A Step Functions `Task` state that asks for the run-a-job (`.sync`) integration
//! pattern over Glue: the Glue job run the task starts is readable through Glue's own
//! API while the execution is parked at it, and the execution's end is that run's.
//!
//! The rule under test: **a `Task` state whose `Resource` ends in `.sync` makes the
//! integration's own starting call, records what came back as the task's submission,
//! and leaves the execution `RUNNING` until the started job reaches a terminal status
//! — and stopping that execution stops the job it started, through the stopping call
//! the integration names.** Two crates have to agree for that to hold (Step Functions
//! starts, reports and stops the job; Glue serves all three calls), so the suite lives
//! here rather than in either of them.
//!
//! The rows differ along what the rule generalises over: the revision of the pattern
//! the `Resource` asks for (`.sync` and `.sync:2`), how the document the state builds
//! names the job (a literal `Parameters` member and one taken from the execution's
//! input with a payload template), and what ends the task (the job's own terminal
//! status, and a `StopExecution` while the task is parked, with and without an `error`
//! and `cause`).
//!
//! Nothing here asserts a value this emulator invented: the job run id comes off the
//! submission event, every expectation about the run is read through `GetJobRun`, and
//! the cause the task fails with is Glue's own report of that run, whose
//! `ErrorMessage` and `JobRunState` are compared with what `GetJobRun` reports.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for, and the one the state machine's ARN
/// names — which is the Region a task's integration call is made in.
const REGION: &str = "us-west-2";

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// How long a poll waits for a status to arrive before giving up.
///
/// A parked task is resolved by polling the job, so the execution's own outcome
/// arrives a few poll intervals after the job's does; this is well past that and far
/// short of the ceiling on a parked job. Generous rather than tight: the chain takes a
/// few seconds on an idle host, and this deadline is only the point at which the work is
/// declared never to have happened — a host running the whole suite at once must not be
/// the thing that trips it.
const PATIENCE: Duration = Duration::from_secs(120);

/// How often a poll asks again.
const POLL: Duration = Duration::from_millis(25);

/// The ASL error name a task whose own work failed reports.
const TASK_FAILED: &str = "States.TaskFailed";

/// The `JobRunState` values a Glue run never moves out of again
/// (`smithy_query(query="shape",shape="JobRunState",model_path="services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json")`
/// lists every value; `STARTING`, `RUNNING`, `STOPPING` and `WAITING` each still owe a
/// further state).
const TERMINAL_RUN_STATES: [&str; 6] = [
    "STOPPED",
    "SUCCEEDED",
    "FAILED",
    "TIMEOUT",
    "ERROR",
    "EXPIRED",
];

/// Step Functions and Glue mounted on ONE engine, as `core/router/src/lib.rs` mounts
/// them. Request authentication is off, as in every other suite here; the in-process
/// callback Step Functions makes signs its request either way, and its credential
/// scope is what routes it to Glue.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is
/// what routes the request and what each handler reads its Region from. The signature
/// is never checked: this router runs with request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one operation of an `awsJson` service and return `(status, body)`.
async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", format!("application/x-amz-json-{json_version}"))
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Call one Step Functions operation (awsJson1_0, `AWSStepFunctions.<Operation>`),
/// requiring a 200.
async fn sfn(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, "states", "AWSStepFunctions", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation}: expected 200, got {status} with {parsed:?}"
    );
    parsed
}

/// Call one Glue operation (awsJson1_1, `AWSGlue.<Operation>`), requiring a 200.
async fn glue(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, "glue", "AWSGlue", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation}: expected 200, got {status} with {parsed:?}"
    );
    parsed
}

/// The string member `name` of `body`, which must be present.
fn string(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {name:?} in {body:?}"))
        .to_string()
}

/// Create the Glue job `name` and return it, so a row's job is its own.
async fn create_job(router: &MultiServiceRouter, name: &str) -> String {
    let created = glue(
        router,
        "CreateJob",
        &json!({
            "Name": name,
            "Role": format!("arn:aws:iam::{ACCOUNT}:role/GlueJobRole"),
            "Command": {
                "Name": "pythonshell",
                "ScriptLocation": format!("s3://sim2real-sfn-glue/{name}.py")
            }
        }),
    )
    .await;
    assert_eq!(
        created.get("Name").and_then(Value::as_str),
        Some(name),
        "setup: CreateJob must answer with the name it stored, got {created:?}"
    );
    name.to_string()
}

/// `DescribeExecution`, as the API answers it.
async fn described(router: &MultiServiceRouter, execution_arn: &str) -> Value {
    sfn(
        router,
        "DescribeExecution",
        &json!({ "executionArn": execution_arn }),
    )
    .await
}

/// The types of the events `GetExecutionHistory` reports, in order.
async fn history_types(router: &MultiServiceRouter, execution_arn: &str) -> Vec<String> {
    let history = sfn(
        router,
        "GetExecutionHistory",
        &json!({ "executionArn": execution_arn }),
    )
    .await;
    history
        .get("events")
        .and_then(Value::as_array)
        .map(|events| {
            events
                .iter()
                .filter_map(|event| event.get("type").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The `JobRunId` the task's `TaskSubmitted` event reports, read off the history
/// rather than guessed at: "the response from a resource when a task has started".
async fn submitted_job_run(router: &MultiServiceRouter, case: &str, execution_arn: &str) -> String {
    let began = Instant::now();
    loop {
        let history = sfn(
            router,
            "GetExecutionHistory",
            &json!({ "executionArn": execution_arn }),
        )
        .await;
        let submitted = history
            .get("events")
            .and_then(Value::as_array)
            .and_then(|events| {
                events
                    .iter()
                    .find(|event| event.get("type").and_then(Value::as_str) == Some("TaskSubmitted"))
                    .cloned()
            });
        if let Some(submitted) = submitted {
            let details = submitted
                .get("taskSubmittedEventDetails")
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected taskSubmittedEventDetails in {submitted:?}")
                });
            let output = string(case, details, "output");
            let reported: Value = serde_json::from_str(&output).unwrap_or_else(|error| {
                panic!("case {case:?}: expected a JSON submission, got {error} in {output:?}")
            });
            return string(case, &reported, "JobRunId");
        }
        assert!(
            began.elapsed() < PATIENCE,
            "case {case:?}: expected the task to submit its job run within {PATIENCE:?}, got the \
             history {:?} and the execution {:?}",
            history_types(router, execution_arn).await,
            described(router, execution_arn).await
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// The run `run_id` of the job `job_name`, as `GetJobRun` reports it.
async fn job_run(router: &MultiServiceRouter, job_name: &str, run_id: &str) -> Value {
    let answered = glue(
        router,
        "GetJobRun",
        &json!({ "JobName": job_name, "RunId": run_id }),
    )
    .await;
    answered
        .get("JobRun")
        .cloned()
        .unwrap_or_else(|| panic!("expected a JobRun for {run_id} of {job_name}, got {answered:?}"))
}

/// The run `run_id` once it has reached a terminal `JobRunState`.
async fn settled_job_run(
    router: &MultiServiceRouter,
    case: &str,
    job_name: &str,
    run_id: &str,
) -> Value {
    let began = Instant::now();
    loop {
        let run = job_run(router, job_name, run_id).await;
        let state = run
            .get("JobRunState")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if TERMINAL_RUN_STATES.contains(&state.as_str()) {
            return run;
        }
        assert!(
            began.elapsed() < PATIENCE,
            "case {case:?}: expected the job run to finish within {PATIENCE:?}, got {run:?}"
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// The execution once it has left `RUNNING`.
async fn settled_execution(router: &MultiServiceRouter, case: &str, execution_arn: &str) -> Value {
    let began = Instant::now();
    loop {
        let described = described(router, execution_arn).await;
        if described.get("status").and_then(Value::as_str) != Some("RUNNING") {
            return described;
        }
        assert!(
            began.elapsed() < PATIENCE,
            "case {case:?}: expected a terminal status within {PATIENCE:?}, got {described:?}"
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// Create the state machine of a row and return its ARN.
async fn create_machine(router: &MultiServiceRouter, name: &str, definition: &Value) -> String {
    let created = sfn(
        router,
        "CreateStateMachine",
        &json!({
            "name": name,
            "definition": definition.to_string(),
            "roleArn": format!("arn:aws:iam::{ACCOUNT}:role/StepFunctionsRole")
        }),
    )
    .await;
    string(name, &created, "stateMachineArn")
}

/// The definition of a single `Task` state asking for the run-a-job pattern over
/// `glue:startJobRun`: `suffix` is the revision it asks for, and `parameters` is the
/// document the state builds, which IS `StartJobRun`'s request.
fn definition(suffix: &str, parameters: Value) -> Value {
    json!({
        "StartAt": "Job",
        "States": {
            "Job": {
                "Type": "Task",
                "Resource": format!("arn:aws:states:::glue:startJobRun{suffix}"),
                "Parameters": parameters,
                "End": true
            }
        }
    })
}

/// One row of the resolution table: how the job is named, and what the task's own
/// result is then resolved by.
struct SyncCase {
    case: &'static str,
    /// The revision of the pattern the `Resource` asks for.
    suffix: &'static str,
    /// Whether the job's name reaches the request through a payload template taken
    /// from the execution's input rather than as a literal member.
    from_input: bool,
    /// The job arguments the state's own document carries, which must be the ones the
    /// started run reports.
    arguments: Value,
}

/// A `.sync` task over Glue starts a real job run, parks the execution at it, and is
/// resolved by what that run ENDED as.
///
/// Every row asserts the whole sequence: the execution is `RUNNING` with no outcome
/// while the run is in flight, the run the `TaskSubmitted` event names is readable
/// through Glue's own API and carries the document the state built, and the task's
/// outcome is that run's own — a run that ended without doing its work fails the task
/// with `States.TaskFailed` carrying the reason Glue gave for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sync_task_over_glue_is_resolved_by_the_job_run_it_started() {
    let rows = [
        SyncCase {
            case: "a literal job name through the pattern's first revision",
            suffix: ".sync",
            from_input: false,
            arguments: json!({ "--marker": "first" }),
        },
        SyncCase {
            case: "the same task through the pattern's second revision",
            suffix: ".sync:2",
            from_input: false,
            arguments: json!({ "--marker": "second" }),
        },
        SyncCase {
            case: "a job name the payload template takes from the execution's input",
            suffix: ".sync",
            from_input: true,
            arguments: json!({}),
        },
    ];

    let router = build_router();
    for (index, row) in rows.iter().enumerate() {
        let job_name = create_job(&router, &format!("sfn-glue-run-{index}")).await;
        let parameters = match row.from_input {
            true => json!({ "JobName.$": "$.job", "Arguments": row.arguments }),
            false => json!({ "JobName": job_name, "Arguments": row.arguments }),
        };
        let machine = create_machine(
            &router,
            &format!("sfn-glue-sync-{index}"),
            &definition(row.suffix, parameters),
        )
        .await;
        let started = sfn(
            &router,
            "StartExecution",
            &json!({
                "stateMachineArn": machine,
                "name": format!("run-{index}"),
                "input": json!({ "job": job_name }).to_string()
            }),
        )
        .await;
        let execution_arn = string(row.case, &started, "executionArn");

        // The task is PARKED: the starting call has answered and the execution has no
        // outcome yet.
        let parked = described(&router, &execution_arn).await;
        assert_eq!(
            parked.get("status").and_then(Value::as_str),
            Some("RUNNING"),
            "case {:?}: expected RUNNING while the job run runs, got {parked:?}",
            row.case
        );
        assert_eq!(
            parked.get("error"),
            None,
            "case {:?}: a parked execution carries no error, got {parked:?}",
            row.case
        );

        // The job run the submission names is Glue's own record of a run of this job,
        // carrying the document the state built as its request.
        let run_id = submitted_job_run(&router, row.case, &execution_arn).await;
        let run = job_run(&router, &job_name, &run_id).await;
        assert_eq!(
            run.get("Id").and_then(Value::as_str),
            Some(run_id.as_str()),
            "case {:?}: expected the run the submission named, got {run:?}",
            row.case
        );
        assert_eq!(
            run.get("JobName").and_then(Value::as_str),
            Some(job_name.as_str()),
            "case {:?}: expected a run of the job the state named, got {run:?}",
            row.case
        );
        let expected_arguments = match row.arguments.as_object().is_some_and(|map| map.is_empty()) {
            // A document with no members is not carried on the wire, so Glue reports
            // the run's arguments as whatever it stored for a request with none.
            true => None,
            false => Some(&row.arguments),
        };
        if let Some(expected) = expected_arguments {
            assert_eq!(
                run.get("Arguments"),
                Some(expected),
                "case {:?}: expected the arguments the state built, got {run:?}",
                row.case
            );
        }

        // The run ends, and what it ended as is the task's own outcome: the cause is
        // GLUE's own report of that very run — the `JobRun` document `GetJobRun`
        // answers with, which the integration reports rather than the wrapper around
        // it — carried as the JSON text of it, so the reason Glue gave is read out of
        // the document rather than being the whole of it.
        let settled_run = settled_job_run(&router, row.case, &job_name, &run_id).await;
        let reported_reason = settled_run
            .get("ErrorMessage")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let settled = settled_execution(&router, row.case, &execution_arn).await;
        assert_eq!(
            settled.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {:?}: the run ended without doing its work ({settled_run:?}), so the task must \
             fail, got {settled:?}",
            row.case
        );
        assert_eq!(
            settled.get("error").and_then(Value::as_str),
            Some(TASK_FAILED),
            "case {:?}: expected {TASK_FAILED}, got {settled:?}",
            row.case
        );
        let cause: Value =
            serde_json::from_str(settled.get("cause").and_then(Value::as_str).unwrap_or(""))
                .unwrap_or_else(|error| {
                    panic!(
                        "case {:?}: expected a JSON cause, got {error} in {settled:?}",
                        row.case
                    )
                });
        assert_eq!(
            cause.get("ErrorMessage").and_then(Value::as_str),
            Some(reported_reason.as_str()),
            "case {:?}: expected Glue's own report of the run as the cause, got {cause:?}",
            row.case
        );
        assert_eq!(
            cause.get("JobRunState").and_then(Value::as_str),
            settled_run.get("JobRunState").and_then(Value::as_str),
            "case {:?}: expected the state Glue reports for the run in the cause, got {cause:?}",
            row.case
        );
        assert_eq!(
            cause.get("Id").and_then(Value::as_str),
            Some(run_id.as_str()),
            "case {:?}: expected the run's own id at the top level of the cause, got {cause:?}",
            row.case
        );
        let types = history_types(&router, &execution_arn).await;
        assert!(
            types.iter().any(|event| event == "TaskSubmitted")
                && types.iter().any(|event| event == "TaskFailed"),
            "case {:?}: expected the submission and the task's failure in {types:?}",
            row.case
        );
    }
}

/// One row of the stop table: the `error` and `cause` the stop carries.
type StopRow = (&'static str, Option<&'static str>, Option<&'static str>);

/// Step Functions, Glue AND Amazon S3 on one engine, for the rows whose job really
/// RUNS its script: the script is an object of the emulated object store, and what the
/// script writes is read back out of it.
fn build_router_with_s3() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(
        engine,
    ));
    router
}

/// The bucket the scripts and what they write live in.
const SCRIPT_BUCKET: &str = "sim2real-sfn-glue-scripts";

/// Make one Amazon S3 request (`restXml`, path-style) and answer `(status, body)`.
async fn s3_request(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Bytes,
) -> (u16, Bytes) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", authorization("s3"))
        .body(body)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (response.status().as_u16(), response.body().clone())
}

/// Create the bucket the scripts live in.
///
/// A regional endpoint takes the location constraint of its own Region, which is what
/// `CreateBucket` refuses a request without.
async fn create_script_bucket(router: &MultiServiceRouter) {
    let configuration = format!(
        "<CreateBucketConfiguration><LocationConstraint>{REGION}</LocationConstraint>\
         </CreateBucketConfiguration>"
    );
    let (status, body) = s3_request(
        router,
        "PUT",
        &format!("/{SCRIPT_BUCKET}"),
        Bytes::from(configuration.into_bytes()),
    )
    .await;
    assert!(
        (200..300).contains(&status),
        "setup: the bucket must be created, got {status} with {body:?}"
    );
}

/// Store `source` as the object `key` and answer the `s3://` location that names it.
async fn stage_script(router: &MultiServiceRouter, key: &str, source: &str) -> String {
    let (status, body) = s3_request(
        router,
        "PUT",
        &format!("/{SCRIPT_BUCKET}/{key}"),
        Bytes::from(source.as_bytes().to_vec()),
    )
    .await;
    assert!(
        (200..300).contains(&status),
        "setup: the script {key} must be stored, got {status} with {body:?}"
    );
    format!("s3://{SCRIPT_BUCKET}/{key}")
}

/// The text of one object, or `None` when the object store does not serve it.
async fn object_text(router: &MultiServiceRouter, key: &str) -> Option<String> {
    let (status, body) = s3_request(
        router,
        "GET",
        &format!("/{SCRIPT_BUCKET}/{key}"),
        Bytes::new(),
    )
    .await;
    if !(200..300).contains(&status) {
        return None;
    }
    String::from_utf8(body.to_vec()).ok()
}

/// Create a Glue job whose `Command` names `location` as its script.
async fn create_job_running(router: &MultiServiceRouter, name: &str, location: &str) {
    let created = glue(
        router,
        "CreateJob",
        &json!({
            "Name": name,
            "Role": format!("arn:aws:iam::{ACCOUNT}:role/GlueJobRole"),
            "Command": {
                "Name": "pythonshell",
                "PythonVersion": "3.9",
                "ScriptLocation": location
            },
            "MaxCapacity": 0.0625,
            "Timeout": 5
        }),
    )
    .await;
    assert_eq!(
        created.get("Name").and_then(Value::as_str),
        Some(name),
        "setup: CreateJob must answer with the name it stored, got {created:?}"
    );
}

/// A `.sync` task over Glue ends as the job's own SCRIPT did, and reports the run's own
/// document as the task's result.
///
/// This is the whole of what the recorded cross-service workflow depends on: the job's
/// script is executed with the arguments the state built, so a script that writes an
/// object and exits cleanly leaves the object behind AND succeeds the execution, while
/// one that raises fails the task with the script's own error. The rows differ along
/// what the script does and along the revision of the pattern the `Resource` asks for.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sync_task_over_glue_ends_as_the_jobs_script_did() {
    /// A script that writes the token its arguments name and exits cleanly.
    const WRITES_ITS_TOKEN: &str = r#"
import sys

import boto3

named = dict(zip(sys.argv[1::2], sys.argv[2::2]))
boto3.client("s3").put_object(
    Bucket=named["--bucket"],
    Key=named["--key"],
    Body=named["--token"].encode("utf-8"),
)
"#;

    /// A script that raises instead.
    const RAISES: &str = r#"
raise ValueError("the partition this run was given is malformed")
"#;

    // (case, the revision asked for, the script, whether it succeeds, the reason a
    //  failing task must carry)
    let rows: [(&str, &str, &str, bool, &str); 3] = [
        (
            "a script that writes its object and exits cleanly",
            ".sync",
            WRITES_ITS_TOKEN,
            true,
            "",
        ),
        (
            "the same script through the pattern's second revision",
            ".sync:2",
            WRITES_ITS_TOKEN,
            true,
            "",
        ),
        (
            "a script that raises",
            ".sync",
            RAISES,
            false,
            "ValueError: the partition this run was given is malformed",
        ),
    ];

    let router = build_router_with_s3();
    create_script_bucket(&router).await;

    for (index, (case, suffix, script, succeeds, reason)) in rows.into_iter().enumerate() {
        let job_name = format!("sfn-glue-script-{index}");
        let token_key = format!("tokens/{job_name}.txt");
        let token = format!("token-{index}");
        let location = stage_script(&router, &format!("{job_name}.py"), script).await;
        create_job_running(&router, &job_name, &location).await;

        let machine = create_machine(
            &router,
            &format!("sfn-glue-script-machine-{index}"),
            &definition(
                suffix,
                json!({
                    "JobName": job_name,
                    "Arguments": {
                        "--bucket": SCRIPT_BUCKET,
                        "--key": token_key,
                        "--token": token,
                    },
                }),
            ),
        )
        .await;
        let started = sfn(
            &router,
            "StartExecution",
            &json!({ "stateMachineArn": machine, "name": format!("script-{index}") }),
        )
        .await;
        let execution_arn = string(case, &started, "executionArn");
        let run_id = submitted_job_run(&router, case, &execution_arn).await;
        let run = settled_job_run(&router, case, &job_name, &run_id).await;
        let settled = settled_execution(&router, case, &execution_arn).await;

        if succeeds {
            assert_eq!(
                run.get("JobRunState").and_then(Value::as_str),
                Some("SUCCEEDED"),
                "case {case:?}: a script that exits cleanly makes the run succeed, got {run:?}"
            );
            assert_eq!(
                run.get("ErrorMessage"),
                None,
                "case {case:?}: a run that failed at nothing carries no ErrorMessage, got {run:?}"
            );
            assert_eq!(
                settled.get("status").and_then(Value::as_str),
                Some("SUCCEEDED"),
                "case {case:?}: the task is resolved by the run that succeeded, got {settled:?}"
            );
            // The task's result IS the run's own document, which is what a `ResultPath`
            // of such a task addresses.
            let output: Value = serde_json::from_str(
                settled.get("output").and_then(Value::as_str).unwrap_or(""),
            )
            .unwrap_or_else(|error| {
                panic!("case {case:?}: expected a JSON output, got {error} in {settled:?}")
            });
            assert_eq!(
                (
                    output.get("Id").and_then(Value::as_str),
                    output.get("JobRunState").and_then(Value::as_str)
                ),
                (Some(run_id.as_str()), Some("SUCCEEDED")),
                "case {case:?}: expected the run's own document as the task's result, got \
                 {output:?}"
            );
            // And the script's own effect is really there: the object it wrote, with the
            // token the state passed as its body.
            let written = object_text(&router, &token_key).await;
            assert_eq!(
                written.as_deref(),
                Some(token.as_str()),
                "case {case:?}: expected the object the script wrote, got {written:?}"
            );
        } else {
            assert_eq!(
                run.get("JobRunState").and_then(Value::as_str),
                Some("FAILED"),
                "case {case:?}: a script that raised makes the run fail, got {run:?}"
            );
            assert_eq!(
                run.get("ErrorMessage").and_then(Value::as_str),
                Some(reason),
                "case {case:?}: expected the script's own error on the run, got {run:?}"
            );
            assert_eq!(
                (
                    settled.get("status").and_then(Value::as_str),
                    settled.get("error").and_then(Value::as_str)
                ),
                (Some("FAILED"), Some(TASK_FAILED)),
                "case {case:?}: expected {TASK_FAILED}, got {settled:?}"
            );
            let cause: Value =
                serde_json::from_str(settled.get("cause").and_then(Value::as_str).unwrap_or(""))
                    .unwrap_or_else(|error| {
                        panic!("case {case:?}: expected a JSON cause, got {error} in {settled:?}")
                    });
            assert_eq!(
                cause.get("ErrorMessage").and_then(Value::as_str),
                Some(reason),
                "case {case:?}: expected the script's own error in the cause, got {cause:?}"
            );
            assert_eq!(
                object_text(&router, &token_key).await,
                None,
                "case {case:?}: a script that raised before writing leaves no object"
            );
        }
    }
}

/// Stopping an execution PARKED at a Glue job run stops that run too, and leaves the
/// execution `ABORTED` with exactly the `error` and `cause` the caller passed.
///
/// The rows vary what the stop carries, because both members are optional: what must
/// not vary is that the run the task started is stopped — Glue reports it `STOPPED`,
/// the status a submitted stop brings a run to rest in, rather than the status it
/// would have reached on its own — and that nothing claims an outcome for the task.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stopping_a_parked_execution_stops_the_glue_job_run_it_started() {
    let rows: [StopRow; 3] = [
        (
            "a stop naming an error and a cause",
            Some("sim2real.Stop"),
            Some("stop while parked"),
        ),
        ("a stop naming only an error", Some("Halt"), None),
        ("a stop naming neither", None, None),
    ];

    let router = build_router();
    for (index, (case, error, cause)) in rows.into_iter().enumerate() {
        let job_name = create_job(&router, &format!("sfn-glue-stop-{index}")).await;
        let machine = create_machine(
            &router,
            &format!("sfn-glue-stopper-{index}"),
            &definition(".sync", json!({ "JobName": job_name })),
        )
        .await;
        let started = sfn(
            &router,
            "StartExecution",
            &json!({ "stateMachineArn": machine, "name": format!("stop-{index}") }),
        )
        .await;
        let execution_arn = string(case, &started, "executionArn");
        let run_id = submitted_job_run(&router, case, &execution_arn).await;

        let mut request = json!({ "executionArn": execution_arn });
        let members = request.as_object_mut().expect("the request is an object");
        if let Some(error) = error {
            members.insert("error".to_string(), json!(error));
        }
        if let Some(cause) = cause {
            members.insert("cause".to_string(), json!(cause));
        }
        let stopped = sfn(&router, "StopExecution", &request).await;
        assert!(
            stopped.get("stopDate").is_some(),
            "case {case:?}: expected a stopDate, got {stopped:?}"
        );

        // (a) the abort crossed the service boundary: the run the task started was
        //     stopped rather than left to finish on its own.
        let run = settled_job_run(&router, case, &job_name, &run_id).await;
        assert_eq!(
            run.get("JobRunState").and_then(Value::as_str),
            Some("STOPPED"),
            "case {case:?}: expected the job run stopped with the execution, got {run:?}"
        );
        // (b) the execution reports the values the caller passed, and nothing else;
        let after = described(&router, &execution_arn).await;
        assert_eq!(
            after.get("status").and_then(Value::as_str),
            Some("ABORTED"),
            "case {case:?}: expected ABORTED, got {after:?}"
        );
        assert_eq!(
            after.get("error").and_then(Value::as_str),
            error,
            "case {case:?}: expected the error the caller passed, got {after:?}"
        );
        assert_eq!(
            after.get("cause").and_then(Value::as_str),
            cause,
            "case {case:?}: expected the cause the caller passed, got {after:?}"
        );
        // (c) the history ends on the abort, with no outcome claimed for the task.
        let types = history_types(&router, &execution_arn).await;
        assert_eq!(
            types.last().map(String::as_str),
            Some("ExecutionAborted"),
            "case {case:?}: expected the history to end on ExecutionAborted, got {types:?}"
        );
        assert!(
            types.iter().any(|event| event == "TaskSubmitted"),
            "case {case:?}: expected the submission of the job run in {types:?}"
        );
        assert!(
            !types
                .iter()
                .any(|event| event == "TaskSucceeded" || event == "TaskFailed"),
            "case {case:?}: a stopped task has neither succeeded nor failed, got {types:?}"
        );
    }
}
