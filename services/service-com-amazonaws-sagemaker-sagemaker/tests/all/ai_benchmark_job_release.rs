// SPDX-License-Identifier: CC0-1.0

//! What it takes to START and to RELEASE an AI benchmark job: the endpoint its
//! `BenchmarkTarget` has to name, the stop that ends a run, and the state gate the delete
//! stands behind.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`, `awsJson1_1`)
//! through the same `MultiServiceRouter` the production router uses, so what these tests
//! exercise is what a real SDK client gets.
//!
//! RULE ONE — `CreateAIBenchmarkJob` resolves its `BenchmarkTarget` before it writes
//! anything: a target naming a SageMaker endpoint this account does not hold is refused with
//! HTTP 400 `ValidationException` "SageMaker endpoint '<identifier>' does not exist",
//! persisting no job, while a target naming one it does hold — by name or by ARN — is
//! accepted.
//!
//! RULE TWO — `DeleteAIBenchmarkJob` releases a job whose run has FINISHED: while the run is
//! still in flight the delete is refused with HTTP 400 `ValidationException` naming the state
//! the job is in and the states a job can be deleted in, and the job survives that refusal;
//! once the job reaches a status nothing leaves on its own the same delete answers the ARN the
//! create minted and the next read of the name is refused `ResourceNotFound`. A delete of a
//! name this account holds no job under is refused `ResourceNotFound` too.
//!
//! RULE THREE — `StopAIBenchmarkJob` ends a running job: it moves into the transient
//! `Stopping` now and the clock owes it `Stopped`, after which there is nothing left to stop
//! and a second stop is refused.
//!
//! The rows span what the rules generalise over: the form the target identifier takes and
//! whether the endpoint behind it exists; and the state the delete is attempted in (a run in
//! flight, a stop in flight, a run that ended on its own, a run the caller stopped, a name
//! nothing created).

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::ai_benchmark_job_lifecycle::seed_target_endpoint;
use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// How long a job is polled for its terminal status before a case fails, and how often.
///
/// What the poll waits for is the CONDITION — the job reaching a status nothing leaves on its
/// own — re-read in a loop, so a loaded host does not fail a case the clock would have served.
const TERMINAL_BUDGET: Duration = Duration::from_secs(60);
const TERMINAL_POLL: Duration = Duration::from_millis(25);

/// `AIBenchmarkJobStatus`'s members nothing leaves on its own
/// (`smithy_query(query="shape",shape="AIBenchmarkJobStatus")`).
const TERMINAL_STATUSES: &[&str] = &["Completed", "Failed", "Stopped"];

/// A `RoleArn` matching the modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/SageMakerBenchmarkRole";

/// The workload configuration every job here references.
const WORKLOAD_CONFIG: &str = "release-workload-config";

/// The endpoint every accepted job here targets.
const ENDPOINT: &str = "release-endpoint";

/// Stand up the workload configuration and the endpoint an accepted job needs.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateAIWorkloadConfig",
            &json!({ "AIWorkloadConfigName": WORKLOAD_CONFIG }),
        )
        .await;
    seed_target_endpoint(&emulator, "us-east-1", ENDPOINT).await;
    emulator
}

/// A `CreateAIBenchmarkJob` body naming `identifier` as its benchmark target.
fn create_body(name: &str, identifier: &str) -> Value {
    json!({
        "AIBenchmarkJobName": name,
        "AIWorkloadConfigIdentifier": WORKLOAD_CONFIG,
        "RoleArn": ROLE_ARN,
        "BenchmarkTarget": { "Endpoint": { "Identifier": identifier } },
        "OutputConfig": { "S3OutputLocation": "s3://bucket/benchmarks/" },
    })
}

/// One row of the target-resolution table.
struct TargetRow {
    case: &'static str,
    /// The `BenchmarkTarget`'s `Identifier`, given the seeded endpoint's name.
    identifier: String,
    /// Whether the create must be accepted.
    accepted: bool,
}

/// The target-resolution table. Rows differ along the FORM the identifier takes (a bare name,
/// the endpoint's ARN) and whether the endpoint behind it exists at all — including an ARN
/// whose resource part names no endpoint, so the reduction to a name is what is resolved
/// rather than the string's shape.
fn target_rows() -> Vec<TargetRow> {
    vec![
        TargetRow {
            case: "the seeded endpoint by name",
            identifier: ENDPOINT.to_string(),
            accepted: true,
        },
        TargetRow {
            case: "the seeded endpoint by ARN",
            identifier: format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:endpoint/{ENDPOINT}"),
            accepted: true,
        },
        TargetRow {
            case: "a name no endpoint was ever created under",
            identifier: "never-created-endpoint".to_string(),
            accepted: false,
        },
        TargetRow {
            case: "an ARN whose resource part names no endpoint",
            identifier: format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:endpoint/absent-endpoint"),
            accepted: false,
        },
        TargetRow {
            case: "a name that is a prefix of the seeded endpoint's",
            identifier: ENDPOINT[..ENDPOINT.len() - 1].to_string(),
            accepted: false,
        },
    ]
}

/// RULE ONE: the benchmark target's endpoint is resolved before any job state is written.
#[tokio::test]
async fn a_benchmark_target_naming_no_endpoint_is_refused_and_persists_nothing() {
    for row in target_rows() {
        let emulator = seeded().await;
        let case = row.case;
        let name = "bench-target";

        let (status, answered) = emulator
            .call("CreateAIBenchmarkJob", &create_body(name, &row.identifier))
            .await;
        if row.accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a target naming an endpoint this account holds must be \
                 accepted, got {status} with {answered}"
            );
            assert_eq!(
                str_field(&answered, "AIBenchmarkJobArn"),
                format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:ai-benchmark-job/{name}"),
                "case {case:?}: the ARN the create answered, got {answered}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?}: an unresolvable target must be refused 400 ValidationException, \
             got {status} with {answered}"
        );
        assert_eq!(
            message(&answered),
            format!("SageMaker endpoint '{}' does not exist", row.identifier),
            "case {case:?}: the refusal must name the identifier the request carried, got \
             {answered}"
        );
        // Nothing was written: the name is still free and no job reads back under it.
        let (read_status, read) = emulator
            .call(
                "DescribeAIBenchmarkJob",
                &json!({ "AIBenchmarkJobName": name }),
            )
            .await;
        assert_eq!(
            (read_status, error_code(&read).as_str()),
            (400, "ResourceNotFound"),
            "case {case:?}: a refused create persists no job, got {read_status} with {read}"
        );
    }
}

/// `DescribeAIBenchmarkJob` for `name`, requiring a 200.
async fn describe(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok(
            "DescribeAIBenchmarkJob",
            &json!({ "AIBenchmarkJobName": name }),
        )
        .await
}

/// Poll the job until it reaches a status nothing leaves on its own, answering that status.
/// Waits for the CONDITION, never for a fixed span.
async fn settled_status(emulator: &Emulator, case: &str, name: &str) -> String {
    let deadline = Instant::now() + TERMINAL_BUDGET;
    loop {
        let described = describe(emulator, name).await;
        let status = str_field(&described, "AIBenchmarkJobStatus");
        if TERMINAL_STATUSES.contains(&status.as_str()) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the job never reached a terminal status; last read {described}"
        );
        tokio::time::sleep(TERMINAL_POLL).await;
    }
}

/// The sentence a delete of a job in `status` is refused with.
fn not_deletable(status: &str) -> String {
    format!(
        "AIBenchmarkJob is in state '{status}' and cannot be deleted. Only Completed, \
         Failed, or Stopped jobs can be deleted."
    )
}

/// RULE TWO: the delete stands behind a state gate, and the refused delete leaves the job
/// intact.
///
/// The non-terminal statuses are reached by acting on the job straight after the call that put
/// it in one — the span the clock gives each move is never zero, which is what makes them
/// observable at all — and the terminal ones by polling for the condition.
#[tokio::test]
async fn a_delete_waits_for_the_run_to_finish_and_then_releases_the_job() {
    // A run still in flight: the delete is refused, naming the state, and the job survives.
    let emulator = seeded().await;
    let running = "bench-running";
    let created = emulator
        .ok("CreateAIBenchmarkJob", &create_body(running, ENDPOINT))
        .await;
    let arn = str_field(&created, "AIBenchmarkJobArn");
    let (status, refused) = emulator
        .call(
            "DeleteAIBenchmarkJob",
            &json!({ "AIBenchmarkJobName": running }),
        )
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case \"a run in flight\": the delete must be refused, got {status} with {refused}"
    );
    assert_eq!(
        message(&refused),
        not_deletable("InProgress"),
        "case \"a run in flight\": the refusal must name the state that stands in the way, \
         got {refused}"
    );
    let survived = describe(&emulator, running).await;
    assert_eq!(
        str_field(&survived, "AIBenchmarkJobArn"),
        arn,
        "case \"a run in flight\": a refused delete leaves the job describable, got {survived}"
    );

    // A stop in flight is non-terminal too, so the delete is refused for it as well.
    let stopping = emulator
        .ok(
            "StopAIBenchmarkJob",
            &json!({ "AIBenchmarkJobName": running }),
        )
        .await;
    assert_eq!(
        str_field(&stopping, "AIBenchmarkJobArn"),
        arn,
        "case \"a stop in flight\": the stop answers the stored ARN, got {stopping}"
    );
    let (status, refused) = emulator
        .call(
            "DeleteAIBenchmarkJob",
            &json!({ "AIBenchmarkJobName": running }),
        )
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case \"a stop in flight\": the delete must be refused, got {status} with {refused}"
    );
    assert_eq!(
        message(&refused),
        not_deletable("Stopping"),
        "case \"a stop in flight\": the refusal must name the state, got {refused}"
    );

    // Once the stop has landed the job is terminal, so the same delete succeeds and the name
    // reads back as held by nothing.
    assert_eq!(
        settled_status(&emulator, "a stopped run", running).await,
        "Stopped",
        "case \"a stopped run\": the clock owes a stopping job the Stopped status"
    );
    let released = emulator
        .ok(
            "DeleteAIBenchmarkJob",
            &json!({ "AIBenchmarkJobName": running }),
        )
        .await;
    assert_eq!(
        str_field(&released, "AIBenchmarkJobArn"),
        arn,
        "case \"a stopped run\": the delete answers the ARN the create minted, got {released}"
    );
    let (status, gone) = emulator
        .call(
            "DescribeAIBenchmarkJob",
            &json!({ "AIBenchmarkJobName": running }),
        )
        .await;
    assert_eq!(
        (status, error_code(&gone).as_str()),
        (400, "ResourceNotFound"),
        "case \"a stopped run\": the released name reads back as held by nothing, got \
         {status} with {gone}"
    );

    // A run left to end on its own is deletable too, once it has ended.
    let left = "bench-left";
    emulator
        .ok("CreateAIBenchmarkJob", &create_body(left, ENDPOINT))
        .await;
    let ended = settled_status(&emulator, "a run left to end", left).await;
    assert!(
        TERMINAL_STATUSES.contains(&ended.as_str()),
        "case \"a run left to end\": the clock must end the run, got {ended:?}"
    );
    emulator
        .ok(
            "DeleteAIBenchmarkJob",
            &json!({ "AIBenchmarkJobName": left }),
        )
        .await;

    // A name nothing created names no job at all.
    let (status, absent) = emulator
        .call(
            "DeleteAIBenchmarkJob",
            &json!({ "AIBenchmarkJobName": "never-created" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&absent).as_str()),
        (400, "ResourceNotFound"),
        "case \"a name nothing created\": the delete must be refused, got {status} with \
         {absent}"
    );
}

/// RULE THREE: the stop ends a running job, and a job whose run is already over has nothing
/// left to stop.
#[tokio::test]
async fn a_stop_ends_a_running_job_and_is_refused_once_the_run_is_over() {
    let emulator = seeded().await;
    let name = "bench-stop";
    emulator
        .ok("CreateAIBenchmarkJob", &create_body(name, ENDPOINT))
        .await;
    emulator
        .ok("StopAIBenchmarkJob", &json!({ "AIBenchmarkJobName": name }))
        .await;
    let stopping = describe(&emulator, name).await;
    assert_eq!(
        str_field(&stopping, "AIBenchmarkJobStatus"),
        "Stopping",
        "case \"a stop just asked for\": the job carries the transient status, got {stopping}"
    );
    assert_eq!(
        settled_status(&emulator, "a stop that landed", name).await,
        "Stopped",
        "case \"a stop that landed\": the clock owes the job the Stopped status"
    );

    let (status, refused) = emulator
        .call("StopAIBenchmarkJob", &json!({ "AIBenchmarkJobName": name }))
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case \"a run already over\": the stop must be refused, got {status} with {refused}"
    );
    assert!(
        message(&refused).contains("is not running"),
        "case \"a run already over\": the refusal must say there is nothing to stop, got \
         {refused}"
    );

    // A name nothing created names no job to stop.
    let (status, absent) = emulator
        .call(
            "StopAIBenchmarkJob",
            &json!({ "AIBenchmarkJobName": "never-created" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&absent).as_str()),
        (400, "ResourceNotFound"),
        "case \"a name nothing created\": the stop must be refused, got {status} with {absent}"
    );
}
