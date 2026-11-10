// SPDX-License-Identifier: CC0-1.0

//! WHEN SageMaker resolves the `RoleArn` a pipeline's steps are performed as: not when the
//! pipeline is registered, but when a run of it needs the role.
//!
//! The rule under test: `CreatePipeline` registers a definition and assumes nothing, so the
//! role is admitted on its modelled shape alone (`RoleArn`, `len 20..2048` and the IAM role
//! ARN pattern — `smithy_query(query="operation",operation="CreatePipeline")`) whatever this
//! account holds under it; the role is resolved by the run that performs a step as it, and a
//! role that resolves to nothing fails that run's step rather than the create.
//!
//! This suite pins the half decided INSIDE the service: a resolution has to be grounded in
//! what IAM ANSWERED, and an emulator with no IAM mounted beside SageMaker — which is
//! exactly this harness — learns nothing about any role, so every row is accepted, stored
//! and run without any run of it reporting a role failure. The other half (a role this
//! account's IAM does answer about, and the `FailureReason` the step then carries) can only
//! be driven with IAM mounted beside this service, so it is asserted in
//! `core/router/tests/all/sagemaker_pipeline_role_resolution.rs`.
//!
//! Rows span the forms the modelled `RoleArn` admits: this account's role, a role under a
//! path, another account's role, a name nothing holds, and the longest value the member's
//! length bound admits.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses. The
//! harness is the one `inference_deployment_lifecycle` declares.

use std::time::Duration;

use serde_json::{json, Value};

use super::inference_deployment_lifecycle::{str_field, Emulator, ACCOUNT};
use super::pipeline_lifecycle::{definition, step, token};

/// Long enough to pass `state::PIPELINE_EXECUTION_ENDS_AFTER` (400 ms), so the run's
/// terminal status is the one the clock already owed.
const PAST_EXECUTION_DUE: Duration = Duration::from_millis(800);

/// A syntactically valid account id that is not the one this emulator serves.
const OTHER_ACCOUNT: &str = "123456789012";

/// The role values every pipeline below is created with.
fn role_values() -> Vec<(&'static str, String)> {
    let prefix = format!("arn:aws:iam::{ACCOUNT}:role/");
    // `RoleArn` is modelled `len 20..2048`, so this is the longest value it admits.
    let longest = format!("{prefix}{}", "r".repeat(2048 - prefix.len()));
    vec![
        ("this account's role", format!("{prefix}pipeline-role")),
        (
            "a role under a path",
            format!("{prefix}service-role/pipeline-role"),
        ),
        (
            "another account's role",
            format!("arn:aws:iam::{OTHER_ACCOUNT}:role/pipeline-role"),
        ),
        ("a name nothing holds", format!("{prefix}never-created")),
        ("the longest value RoleArn admits", longest),
    ]
}

/// A `CreatePipeline` body for `name` carrying `role`, with one step to run.
fn create_body(name: &str, role: &str) -> Value {
    json!({
        "PipelineName": name,
        "PipelineDefinition": definition(json!([step("Only", &[])])),
        "ClientRequestToken": token(name),
        "RoleArn": role,
    })
}

/// RULE: a pipeline is registered around whatever well-formed `RoleArn` the request names —
/// the create resolves no role — and a run of it reports no role failure while no IAM has
/// answered anything about that role.
#[tokio::test]
async fn a_pipeline_is_registered_around_whatever_role_the_request_names() {
    let emulator = Emulator::new();

    for (index, (case, role)) in role_values().iter().enumerate() {
        let name = format!("roleprobe{index}");
        let (status, created) = emulator.call("CreatePipeline", &create_body(&name, role)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the create resolves no role, so it must be accepted, got {status} \
             with {created}"
        );
        assert_eq!(
            str_field(&created, "PipelineArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:pipeline/{name}"),
            "case {case:?}: PipelineArn, got {created}"
        );

        let (status, read) = emulator
            .call("DescribePipeline", &json!({ "PipelineName": name }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the accepted create must store the pipeline, got {status} with {read}"
        );
        assert_eq!(
            str_field(&read, "RoleArn"),
            *role,
            "case {case:?}: the stored pipeline must carry the role it was given, got {read}"
        );

        // A run of it is accepted too, and nothing about the role is reported on it: with
        // no IAM to ask, no role resolved to nothing.
        let (status, started) = emulator
            .call(
                "StartPipelineExecution",
                &json!({ "PipelineName": name, "ClientRequestToken": token(&format!("run{index}")) }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the run must be accepted, got {status} with {started}"
        );
        let execution = str_field(&started, "PipelineExecutionArn");

        tokio::time::sleep(PAST_EXECUTION_DUE).await;
        let (status, settled) = emulator
            .call(
                "DescribePipelineExecution",
                &json!({ "PipelineExecutionArn": execution }),
            )
            .await;
        assert_eq!(
            (status, str_field(&settled, "PipelineExecutionStatus").as_str()),
            (200, "Failed"),
            "case {case:?}: the run's terminal status, got {status} with {settled}"
        );
        let reason = str_field(&settled, "FailureReason");
        assert!(
            !reason.contains(role.as_str()),
            "case {case:?}: with no IAM to ask, the run must not blame the role, got {reason:?}"
        );

        let (status, steps) = emulator
            .call(
                "ListPipelineExecutionSteps",
                &json!({ "PipelineExecutionArn": execution }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the run's steps must be listable, got {status} with {steps}"
        );
        let listed = steps
            .get("PipelineExecutionSteps")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected a step list in {steps}"));
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: the definition declares one step, got {steps}"
        );
        let step_reason = str_field(&listed[0], "FailureReason");
        assert!(
            !step_reason.contains(role.as_str()),
            "case {case:?}: with no IAM to ask, the step must not blame the role, got \
             {step_reason:?}"
        );
    }
}
