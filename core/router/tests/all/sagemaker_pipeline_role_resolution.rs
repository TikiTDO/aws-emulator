// SPDX-License-Identifier: CC0-1.0

//! WHEN SageMaker resolves the role a pipeline's steps are performed as, and what it
//! reports when that role resolves to nothing.
//!
//! The rules under test:
//!
//! * `CreatePipeline` registers a definition and assumes nothing, so it is accepted HTTP
//!   200 whatever this account's IAM holds under the `RoleArn` it names — the role is
//!   resolved by the RUN that performs a step as it;
//! * a run whose role this account's IAM answers against is started just the same, and the
//!   failure travels on the run and on the step that needed the role: the execution reaches
//!   the modelled `Failed` status and the step reports the `FailureReason` that states what
//!   IAM answered — that no such role exists, or that the role cannot be assumed;
//! * because the resolution happens at the run, a pipeline registered before its role
//!   exists runs successfully past role resolution once the role is created.
//!
//! The rules reach ANOTHER service (IAM) through the in-process callback, so a
//! single-service router has nothing to ask and this suite needs the whole router — which
//! is why it lives here rather than in the SageMaker crate, whose
//! `tests/all/pipeline_role_resolution.rs` pins the half decided inside the service (with no
//! IAM to ask, nothing is refused and nothing is blamed on the role).
//!
//! The rows span what the rules generalise over: whether IAM holds the role at all, the
//! shape and the effect of the trust document it holds, which account the ARN names, and —
//! for the second rule — the instant the role is created relative to the pipeline.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;
use std::time::Duration;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};

use crate::sagemaker_create_admission::{
    create_role, sagemaker, trust_policy, RoleFixture, ACCOUNT, BOOT_STACK_SIZE, OTHER_ACCOUNT,
};

/// Long enough to pass `state::PIPELINE_EXECUTION_ENDS_AFTER` (400 ms), so a run's terminal
/// status and its steps are the ones the clock already owed.
const PAST_EXECUTION_DUE: Duration = Duration::from_millis(800);

/// The one step every pipeline below declares, named for the work the recorded scenario
/// gives it: a callback step, whose side effect is a message on the caller's queue.
const STEP_NAME: &str = "CallbackStep";

/// What this account's IAM answers about one role, and so what a run under it must report.
enum Answer {
    /// IAM holds no role under that name: the run's step is told no such role exists.
    Absent,
    /// IAM holds the role and its trust document withholds the SageMaker principal.
    Withheld,
    /// Nothing is learned about the role, so no run is blamed on it.
    Nothing,
}

/// One role a pipeline is registered around.
struct RoleRow {
    case: &'static str,
    /// The role to hold in IAM first: its name, its path, and its trust document.
    fixture: Option<RoleFixture>,
    /// The value the `CreatePipeline` request carries.
    role_arn: String,
    answer: Answer,
}

impl RoleRow {
    /// The `FailureReason` a run of a pipeline under this role must report, or `None` where
    /// IAM answered nothing and the run fails for its own reason instead.
    fn expected_reason(&self) -> Option<String> {
        match self.answer {
            Answer::Absent => Some(format!("Role {} does not exist.", self.role_arn)),
            Answer::Withheld => Some(format!("Unable to assume role with RoleArn {}", self.role_arn)),
            Answer::Nothing => None,
        }
    }
}

/// The role values the pipelines below are registered around: trust documents differing in
/// the shape AWS accepts for `Action` and `Principal.Service` (one string, or a list), in
/// their effect and in the principal they name; a role under a path; a role IAM holds
/// nothing under; and a role of another account.
fn role_rows() -> Vec<RoleRow> {
    vec![
        RoleRow {
            case: "a role that trusts SageMaker",
            fixture: Some(RoleFixture {
                role_name: "pipeline-exec",
                path: None,
                policy: trust_policy("Allow", "\"sts:AssumeRole\"", "\"sagemaker.amazonaws.com\""),
            }),
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/pipeline-exec"),
            answer: Answer::Nothing,
        },
        RoleRow {
            case: "a role whose trust document states its action and principal as lists",
            fixture: Some(RoleFixture {
                role_name: "pipeline-exec-lists",
                path: None,
                policy: trust_policy(
                    "Allow",
                    "[\"sts:TagSession\",\"sts:AssumeRole\"]",
                    "[\"glue.amazonaws.com\",\"sagemaker.amazonaws.com\"]",
                ),
            }),
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/pipeline-exec-lists"),
            answer: Answer::Nothing,
        },
        RoleRow {
            case: "a role under a path",
            fixture: Some(RoleFixture {
                role_name: "pipeline-path-exec",
                path: Some("/service-role/"),
                policy: trust_policy("Allow", "\"sts:AssumeRole\"", "\"sagemaker.amazonaws.com\""),
            }),
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/service-role/pipeline-path-exec"),
            answer: Answer::Nothing,
        },
        RoleRow {
            case: "a role that trusts another service only",
            fixture: Some(RoleFixture {
                role_name: "pipeline-lambda-exec",
                path: None,
                policy: trust_policy("Allow", "\"sts:AssumeRole\"", "\"lambda.amazonaws.com\""),
            }),
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/pipeline-lambda-exec"),
            answer: Answer::Withheld,
        },
        RoleRow {
            case: "a role whose statement denies the assume",
            fixture: Some(RoleFixture {
                role_name: "pipeline-denied-exec",
                path: None,
                policy: trust_policy("Deny", "\"sts:AssumeRole\"", "\"sagemaker.amazonaws.com\""),
            }),
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/pipeline-denied-exec"),
            answer: Answer::Withheld,
        },
        RoleRow {
            case: "a role this account never created",
            fixture: None,
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/pipeline-never-created"),
            answer: Answer::Absent,
        },
        RoleRow {
            // Another account's IAM is not this emulator's to ask, so nothing is learned
            // about the role and no run is blamed on it.
            case: "a role of another account",
            fixture: None,
            role_arn: format!("arn:aws:iam::{OTHER_ACCOUNT}:role/pipeline-exec"),
            answer: Answer::Nothing,
        },
    ]
}

/// A `ClientRequestToken` padded to the modelled minimum of 32 characters
/// (`IdempotencyToken`, len 32..128).
fn token(seed: &str) -> String {
    format!("{seed:-<32}")
}

/// A `CreatePipeline` body for `name` carrying `role`: one callback step, in the only
/// published definition schema version.
fn create_body(name: &str, role: &str) -> Value {
    json!({
        "PipelineName": name,
        "ClientRequestToken": token(name),
        "RoleArn": role,
        "PipelineDefinition": json!({
            "Version": "2020-12-01",
            "Metadata": {},
            "Steps": [{
                "Name": STEP_NAME,
                "Type": "Callback",
                "SqsQueueUrl": "https://sqs.us-east-1.amazonaws.com/queue/callbacks",
                "Arguments": { "Marker": name },
            }],
        })
        .to_string(),
    })
}

/// Start a run of `pipeline` under `request_token` and answer, once the clock has settled
/// it, its `PipelineExecutionStatus` and the `FailureReason` its sole step reports.
fn run_and_settle(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    pipeline: &str,
    request_token: &str,
) -> (String, Option<String>) {
    let (status, started) = sagemaker(
        router,
        runtime,
        "StartPipelineExecution",
        &json!({ "PipelineName": pipeline, "ClientRequestToken": token(request_token) }),
    );
    assert_eq!(
        status, 200,
        "case {case:?}: the run must be accepted whatever the role, got {status} {started}"
    );
    let execution = started
        .get("PipelineExecutionArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a PipelineExecutionArn in {started}"))
        .to_string();

    std::thread::sleep(PAST_EXECUTION_DUE);
    let (status, settled) = sagemaker(
        router,
        runtime,
        "DescribePipelineExecution",
        &json!({ "PipelineExecutionArn": execution }),
    );
    assert_eq!(
        status, 200,
        "case {case:?}: the started run must be readable, got {status} {settled}"
    );
    let execution_status = settled
        .get("PipelineExecutionStatus")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let (status, listed) = sagemaker(
        router,
        runtime,
        "ListPipelineExecutionSteps",
        &json!({ "PipelineExecutionArn": execution }),
    );
    assert_eq!(
        status, 200,
        "case {case:?}: the run's steps must be listable, got {status} {listed}"
    );
    let steps = listed
        .get("PipelineExecutionSteps")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a step list in {listed}"))
        .clone();
    assert_eq!(
        steps.len(),
        1,
        "case {case:?}: the definition declares one step, got {listed}"
    );
    assert_eq!(
        steps[0].get("StepName").and_then(Value::as_str),
        Some(STEP_NAME),
        "case {case:?}: the listed step must be the declared one, got {listed}"
    );
    (
        execution_status,
        steps[0]
            .get("FailureReason")
            .and_then(Value::as_str)
            .map(str::to_string),
    )
}

/// RULE: a pipeline is registered around whatever well-formed `RoleArn` the request names,
/// and the role is resolved by the run instead: a run whose role this account's IAM answers
/// against ends in the modelled `Failed` status with its step reporting what IAM answered —
/// that no such role exists, or that the role cannot be assumed — while a run whose role IAM
/// is content with, or says nothing about, is not blamed on the role at all.
#[test]
fn a_pipeline_resolves_its_role_when_a_run_needs_it() {
    std::thread::Builder::new()
        .name("sagemaker-pipeline-role-resolution".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            let rows = role_rows();
            for row in &rows {
                if let Some(fixture) = &row.fixture {
                    create_role(&router, &runtime, fixture);
                }
            }

            for (index, row) in rows.iter().enumerate() {
                let case = row.case;
                let name = format!("rolepipe{index}");
                let (status, created) =
                    sagemaker(&router, &runtime, "CreatePipeline", &create_body(&name, &row.role_arn));
                assert_eq!(
                    status, 200,
                    "case {case:?}: registering a pipeline resolves no role, so it must be \
                     accepted, got {status} {created}"
                );
                let (status, read) = sagemaker(
                    &router,
                    &runtime,
                    "DescribePipeline",
                    &json!({ "PipelineName": name }),
                );
                assert_eq!(
                    (status, read.get("RoleArn").and_then(Value::as_str)),
                    (200, Some(row.role_arn.as_str())),
                    "case {case:?}: the stored pipeline must carry the role it was given, got \
                     {status} {read}"
                );

                let (execution_status, step_reason) =
                    run_and_settle(&router, &runtime, case, &name, &format!("run{index}"));
                assert_eq!(
                    execution_status, "Failed",
                    "case {case:?}: the run's terminal status, got {execution_status:?}"
                );
                match row.expected_reason() {
                    Some(expected) => assert_eq!(
                        step_reason.as_deref(),
                        Some(expected.as_str()),
                        "case {case:?}: the step must report what IAM answered about the role, \
                         got {step_reason:?}"
                    ),
                    None => assert!(
                        step_reason
                            .as_deref()
                            .is_none_or(|reason| !reason.contains(&row.role_arn)),
                        "case {case:?}: no run may be blamed on a role IAM answered nothing \
                         against, got {step_reason:?}"
                    ),
                }
            }
        })
        .expect("a sized thread")
        .join()
        .expect("the suite thread");
}

/// RULE: because the role is resolved by the run and not by the create, a pipeline
/// registered BEFORE its role exists is told the role is missing only for as long as it is
/// missing: a run started after the role is created is not blamed on it.
#[test]
fn a_role_created_after_the_pipeline_resolves_for_the_next_run() {
    std::thread::Builder::new()
        .name("sagemaker-pipeline-role-created-later".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // The deploy ordering the rule is about: the pipeline is registered first, and
            // the role it names is created afterwards.
            let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/deployed-later");
            let name = "laterrolepipe";
            let (status, created) =
                sagemaker(&router, &runtime, "CreatePipeline", &create_body(name, &role_arn));
            assert_eq!(
                status, 200,
                "case before-the-role: the create must be accepted, got {status} {created}"
            );

            let (execution_status, step_reason) =
                run_and_settle(&router, &runtime, "before-the-role", name, "runbefore");
            assert_eq!(
                (execution_status.as_str(), step_reason.as_deref()),
                ("Failed", Some(format!("Role {role_arn} does not exist.").as_str())),
                "case before-the-role: the step must report the missing role, got \
                 {execution_status:?} {step_reason:?}"
            );

            create_role(
                &router,
                &runtime,
                &RoleFixture {
                    role_name: "deployed-later",
                    path: None,
                    policy: trust_policy(
                        "Allow",
                        "\"sts:AssumeRole\"",
                        "\"sagemaker.amazonaws.com\"",
                    ),
                },
            );

            let (_, step_reason) =
                run_and_settle(&router, &runtime, "after-the-role", name, "runafter");
            assert!(
                step_reason
                    .as_deref()
                    .is_none_or(|reason| !reason.contains(&role_arn)),
                "case after-the-role: a run started once the role exists must not be blamed on \
                 it, got {step_reason:?}"
            );
        })
        .expect("a sized thread")
        .join()
        .expect("the suite thread");
}
