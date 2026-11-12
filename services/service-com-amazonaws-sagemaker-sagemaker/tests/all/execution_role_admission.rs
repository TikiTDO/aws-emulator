// SPDX-License-Identifier: CC0-1.0

//! What a SageMaker create does with a caller-supplied execution role when there is NO IAM
//! to ask about it.
//!
//! The role rule has two halves, and this suite pins the half that is decided inside this
//! service: a refusal has to be grounded in what IAM ANSWERED, so an emulator with no IAM
//! mounted beside SageMaker — which is exactly this harness — refuses NOTHING, whatever the
//! role value names. The other half (the refusal itself, and the `CREATE_FAILED` a custom
//! image settles into) can only be driven with IAM mounted beside this service, so it is
//! asserted in `core/router/tests/all/sagemaker_execution_role_admission.rs`.
//!
//! Without this suite a guard that refused on an answer it never got would pass every
//! cross-service case and make the whole create surface of a single-service emulator
//! unreachable.
//!
//! Rows span what the rule generalises over: the create that takes the role (five, each of
//! a different resource family and one of them taking a role per instance group), and the
//! role value's form — this account's, under a path, another account's, a name nothing
//! holds, and the longest value the modelled `RoleArn` admits.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses. The
//! harness is the one `inference_deployment_lifecycle` declares.

use std::time::Duration;

use serde_json::{json, Value};

use crate::ai_benchmark_job_lifecycle::seed_target_endpoint;
use crate::inference_deployment_lifecycle::{str_field, Emulator, ACCOUNT};

/// Long enough to pass `state::IMAGE_CREATED_AFTER` (300 ms), so the image's settled status
/// is the one the clock already owed.
const PAST_IMAGE_SETTLE_DUE: Duration = Duration::from_millis(500);

/// The AI workload configuration the benchmark job below is driven against.
const WORKLOAD_CONFIG: &str = "roleprobeconfig";

/// The endpoint the `CreateAIBenchmarkJob` row below targets: that create resolves its
/// `BenchmarkTarget` against the endpoint store, so the endpoint is seeded with the
/// workload configuration.
const TARGET_ENDPOINT: &str = "roleprobeendpoint";

/// A syntactically valid account id that is not the one this emulator serves.
const OTHER_ACCOUNT: &str = "123456789012";

/// The role values every create below is driven with: the forms the modelled `RoleArn`
/// pattern `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$` admits
/// (smithy_query operation=CreateAIBenchmarkJob), including its `len 20..2048` upper bound.
fn role_values() -> Vec<(&'static str, String)> {
    let prefix = format!("arn:aws:iam::{ACCOUNT}:role/");
    let longest = format!("{prefix}{}", "r".repeat(2048 - prefix.len()));
    vec![
        ("this account's role", format!("{prefix}sagemaker-exec")),
        (
            "a role under a path",
            format!("{prefix}service-role/sagemaker-exec"),
        ),
        (
            "another account's role",
            format!("arn:aws:iam::{OTHER_ACCOUNT}:role/sagemaker-exec"),
        ),
        ("a name nothing holds", format!("{prefix}never-created")),
        ("the longest value RoleArn admits", longest),
    ]
}

/// One create driven for every role value, with the reader that proves what it stored.
struct Create {
    operation: &'static str,
    body: fn(&str, &str) -> Value,
    reader: &'static str,
    read_body: fn(&str) -> Value,
    /// The member of the reader's answer that must carry the role back.
    reported_role: &'static str,
}

/// A minimal HyperPod instance group assuming `role`.
fn instance_group(name: &str, role: &str) -> Value {
    json!({
        "InstanceGroupName": name,
        "InstanceCount": 1,
        "InstanceType": "ml.c5.xlarge",
        "ExecutionRole": role,
    })
}

/// The creates this suite drives: one per resource family that resolves a role.
fn creates() -> Vec<Create> {
    vec![
        Create {
            operation: "CreateAIBenchmarkJob",
            body: |name, role| {
                json!({
                    "AIBenchmarkJobName": name,
                    "AIWorkloadConfigIdentifier": WORKLOAD_CONFIG,
                    "RoleArn": role,
                    "BenchmarkTarget": { "Endpoint": { "Identifier": TARGET_ENDPOINT } },
                    "OutputConfig": { "S3OutputLocation": "s3://bucket/benchmarks/" },
                })
            },
            reader: "DescribeAIBenchmarkJob",
            read_body: |name| json!({ "AIBenchmarkJobName": name }),
            reported_role: "RoleArn",
        },
        Create {
            operation: "CreateMlflowApp",
            body: |name, role| {
                json!({
                    "Name": name,
                    "ArtifactStoreUri": "s3://mlflow-artifacts/apps/",
                    "RoleArn": role,
                })
            },
            // Addressed by the ARN the create mints, which the loop substitutes for the
            // name before it reads.
            reader: "DescribeMlflowApp",
            read_body: |arn| json!({ "Arn": arn }),
            reported_role: "RoleArn",
        },
        Create {
            operation: "CreateModel",
            body: |name, role| {
                json!({
                    "ModelName": name,
                    "ExecutionRoleArn": role,
                    "PrimaryContainer": {
                        "Image": format!("{ACCOUNT}.dkr.ecr.us-east-1.amazonaws.com/infer:1"),
                    },
                })
            },
            reader: "DescribeModel",
            read_body: |name| json!({ "ModelName": name }),
            reported_role: "ExecutionRoleArn",
        },
        Create {
            operation: "CreateImage",
            body: |name, role| json!({ "ImageName": name, "RoleArn": role }),
            reader: "DescribeImage",
            read_body: |name| json!({ "ImageName": name }),
            reported_role: "RoleArn",
        },
    ]
}

/// RULE: with no IAM mounted to answer about the role, every create that takes one is
/// accepted and stores the resource carrying the role it was given — a refusal has to be
/// grounded in an answer, and there is none here.
#[tokio::test]
async fn a_create_refuses_no_role_when_no_iam_answers_about_it() {
    let emulator = Emulator::new();
    let (status, seeded) = emulator
        .call(
            "CreateAIWorkloadConfig",
            &json!({ "AIWorkloadConfigName": WORKLOAD_CONFIG }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: CreateAIWorkloadConfig must be accepted, got {status} with {seeded}"
    );
    seed_target_endpoint(&emulator, "us-east-1", TARGET_ENDPOINT).await;

    for (index, (case, role)) in role_values().iter().enumerate() {
        for create in creates() {
            let name = format!("r{index}{}", create.operation.to_lowercase());
            let case = format!("{} / {case}", create.operation);
            let (status, created) = emulator
                .call(create.operation, &(create.body)(&name, role))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: with no IAM to ask, the create must be accepted, got {status} \
                 with {created}"
            );

            // `CreateMlflowApp` mints the id its reader takes; every other create here is
            // addressed by the name the request gave it.
            let selector = if create.operation == "CreateMlflowApp" {
                str_field(&created, "Arn")
            } else {
                name.clone()
            };
            let (status, read) = emulator
                .call(create.reader, &(create.read_body)(&selector))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the accepted create must store the resource, got {status} with \
                 {read}"
            );
            assert_eq!(
                str_field(&read, create.reported_role),
                *role,
                "case {case:?}: the stored resource must carry the role it was given, got {read}"
            );
        }

        // The two creates that take a role in a member of their own: one per instance group
        // of a cluster, and one per training job definition of a tuning job.
        let cluster = format!("r{index}cluster");
        let (status, created) = emulator
            .call(
                "CreateCluster",
                &json!({
                    "ClusterName": cluster,
                    "InstanceGroups": [
                        instance_group("head", role),
                        instance_group("workers", role),
                    ],
                    "ClusterRole": role,
                    "AutoScaling": { "Mode": "Enable", "AutoScalerType": "Karpenter" },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case CreateCluster / {case:?}: with no IAM to ask, the create must be accepted, \
             got {status} with {created}"
        );
        let (status, read) = emulator
            .call("DescribeCluster", &json!({ "ClusterName": cluster }))
            .await;
        assert_eq!(
            status, 200,
            "case CreateCluster / {case:?}: the accepted create must store the cluster, got \
             {status} with {read}"
        );
        let groups = read
            .get("InstanceGroups")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case CreateCluster / {case:?}: expected groups in {read}"));
        for group in groups {
            assert_eq!(
                str_field(group, "ExecutionRole"),
                *role,
                "case CreateCluster / {case:?}: every group must keep the role it declared, got \
                 {read}"
            );
        }

        let tuning = format!("r{index}tuning");
        let (status, created) = emulator
            .call(
                "CreateHyperParameterTuningJob",
                &json!({
                    "HyperParameterTuningJobName": tuning,
                    "HyperParameterTuningJobConfig": {
                        "Strategy": "Bayesian",
                        "ResourceLimits": {
                            "MaxNumberOfTrainingJobs": 4,
                            "MaxParallelTrainingJobs": 2,
                        },
                        "TrainingJobEarlyStoppingType": "Off",
                    },
                    "TrainingJobDefinition": {
                        "AlgorithmSpecification": {
                            "TrainingImage": format!(
                                "{ACCOUNT}.dkr.ecr.us-east-1.amazonaws.com/xgboost:1"
                            ),
                            "TrainingInputMode": "File",
                        },
                        "RoleArn": role,
                        "OutputDataConfig": { "S3OutputPath": "s3://tuning-output/artifacts/" },
                        "ResourceConfig": {
                            "InstanceType": "ml.m5.large",
                            "InstanceCount": 1,
                            "VolumeSizeInGB": 10,
                        },
                        "StoppingCondition": { "MaxRuntimeInSeconds": 3600 },
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case CreateHyperParameterTuningJob / {case:?}: with no IAM to ask, the create must \
             be accepted, got {status} with {created}"
        );
        let (status, read) = emulator
            .call(
                "DescribeHyperParameterTuningJob",
                &json!({ "HyperParameterTuningJobName": tuning }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case CreateHyperParameterTuningJob / {case:?}: the accepted create must store the \
             job, got {status} with {read}"
        );
        assert_eq!(
            read.pointer("/TrainingJobDefinition/RoleArn")
                .and_then(Value::as_str),
            Some(role.as_str()),
            "case CreateHyperParameterTuningJob / {case:?}: the stored definition must keep its \
             role, got {read}"
        );
    }
}

/// RULE: an image whose creation resolved nothing against it settles into `CREATED` with no
/// `FailureReason` — the modelled `CREATE_FAILED` is reserved for a create whose role this
/// emulator's own IAM answered about, which this harness has none of.
#[tokio::test]
async fn an_image_settles_created_when_no_iam_answers_about_its_role() {
    let emulator = Emulator::new();

    for (index, (case, role)) in role_values().iter().enumerate() {
        let name = format!("image{index}");
        let (status, created) = emulator
            .call("CreateImage", &json!({ "ImageName": name, "RoleArn": role }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateImage must be accepted, got {status} with {created}"
        );

        let (status, creating) = emulator
            .call("DescribeImage", &json!({ "ImageName": name }))
            .await;
        assert_eq!(
            (status, str_field(&creating, "ImageStatus").as_str()),
            (200, "CREATING"),
            "case {case:?}: a freshly created image must be reported CREATING, got {status} with \
             {creating}"
        );

        tokio::time::sleep(PAST_IMAGE_SETTLE_DUE).await;
        let (status, settled) = emulator
            .call("DescribeImage", &json!({ "ImageName": name }))
            .await;
        assert_eq!(
            (status, str_field(&settled, "ImageStatus").as_str()),
            (200, "CREATED"),
            "case {case:?}: the image must settle CREATED, got {status} with {settled}"
        );
        assert!(
            settled.get("FailureReason").is_none(),
            "case {case:?}: an image that failed at nothing carries no FailureReason, got \
             {settled}"
        );
    }
}
