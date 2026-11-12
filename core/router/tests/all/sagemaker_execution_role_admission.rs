// SPDX-License-Identifier: CC0-1.0

//! What SageMaker does with a caller-supplied role the SERVICE has to assume: the create
//! resolves it against this account's IAM before it stores anything, and answers as the
//! resource's own family answers.
//!
//! The rules under test:
//!
//! * a create whose resource runs as, or is built under, a caller-supplied role is refused
//!   HTTP 400 `ValidationException` — storing nothing — when this account's IAM answers
//!   that SageMaker cannot assume that role, with the sentence that resource family words
//!   the fact in; the operations that follow such a refusal then report the resource as NOT
//!   FOUND, because the refused create stored none;
//! * where the service ACCEPTS the create instead and reports the failure on the resource
//!   (a custom image), the resource settles into its modelled `CREATE_FAILED` status
//!   carrying the `FailureReason` that names the role, rather than reaching a ready status.
//!
//! Both rules reach ANOTHER service (IAM) through the in-process callback, so a
//! single-service router has nothing to ask and this suite needs the whole router — which
//! is why it lives here rather than in the SageMaker crate. The sibling suite
//! `sagemaker_create_admission` drives the same role rule over the job family; this one
//! drives the families whose recorded refusal is worded differently, the creates that take
//! SEVERAL roles, the operations that follow a refused create, and the create that answers
//! on the resource.
//!
//! The rows span what the rules generalise over: the operation that takes the role (and so
//! the sentence and the reading it answers with), the shape and the effect of the trust
//! document IAM holds, whether IAM holds the role at all, which account the ARN names,
//! which member of a multi-role request carries the role, and — for the image — the instant
//! the status is read at.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;
use std::time::Duration;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};

use crate::sagemaker_create_admission::{
    error_code, error_message, iam, sagemaker, trust_policy, ACCOUNT, BOOT_STACK_SIZE,
    OTHER_ACCOUNT,
};

/// Long enough to pass `state::IMAGE_CREATED_AFTER` (300 ms), so the image's settled status
/// is the one the clock already owed.
const PAST_IMAGE_SETTLE_DUE: Duration = Duration::from_millis(500);

/// The AI workload configuration every benchmark job below is driven against, so a refusal
/// cannot be the missing-reference one instead.
const WORKLOAD_CONFIG: &str = "rolebenchconfig";

/// The inference endpoint the `CreateAIBenchmarkJob` row's `BenchmarkTarget` names: that
/// create resolves the target against the endpoint store, so the endpoint is seeded with the
/// workload configuration.
const BENCHMARK_ENDPOINT: &str = "rolebenchendpoint";

/// One role value the creates are driven with, and what this account's IAM answers about
/// it.
struct RoleRow {
    case: &'static str,
    /// The role to hold in IAM first: its name, its path, and its trust document.
    fixture: Option<RoleFixture>,
    /// The value the request carries.
    role_arn: String,
    /// Whether the value names a role of THIS account, i.e. one this emulator's IAM can be
    /// asked about at all.
    this_account: bool,
    /// Whether the trust document the held role carries admits the SageMaker principal.
    assumable: bool,
}

/// One role IAM is made to hold before the creates run.
struct RoleFixture {
    role_name: &'static str,
    path: Option<&'static str>,
    policy: String,
}

impl RoleRow {
    /// Whether a create that refuses a role IAM holds NOTHING under accepts this row.
    fn accepted_when_absent_is_refused(&self) -> bool {
        !self.this_account || (self.fixture.is_some() && self.assumable)
    }

    /// Whether a create that refuses only what a STORED trust document withholds accepts
    /// this row.
    fn accepted_when_only_a_held_role_is_refused(&self) -> bool {
        !self.this_account || self.fixture.is_none() || self.assumable
    }
}

/// The role values every create below is driven with: trust documents differing in the
/// shape AWS accepts for `Action` and `Principal.Service` (one string, or a list), in their
/// effect, in the action they allow and in the principal they name; a role under a path; a
/// role IAM holds nothing under; and a role of another account.
fn role_rows() -> Vec<RoleRow> {
    vec![
        RoleRow {
            case: "a role that trusts SageMaker",
            fixture: Some(RoleFixture {
                role_name: "sagemaker-exec",
                path: None,
                policy: trust_policy("Allow", "\"sts:AssumeRole\"", "\"sagemaker.amazonaws.com\""),
            }),
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/sagemaker-exec"),
            this_account: true,
            assumable: true,
        },
        RoleRow {
            case: "a role whose trust document states its action and principal as lists",
            fixture: Some(RoleFixture {
                role_name: "sagemaker-exec-lists",
                path: None,
                policy: trust_policy(
                    "Allow",
                    "[\"sts:TagSession\",\"sts:AssumeRole\"]",
                    "[\"glue.amazonaws.com\",\"sagemaker.amazonaws.com\"]",
                ),
            }),
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/sagemaker-exec-lists"),
            this_account: true,
            assumable: true,
        },
        RoleRow {
            case: "a role under a path",
            fixture: Some(RoleFixture {
                role_name: "sagemaker-exec-scoped",
                path: Some("/service-role/"),
                policy: trust_policy("Allow", "\"sts:AssumeRole\"", "\"sagemaker.amazonaws.com\""),
            }),
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/service-role/sagemaker-exec-scoped"),
            this_account: true,
            assumable: true,
        },
        RoleRow {
            case: "a role that trusts another service only",
            fixture: Some(RoleFixture {
                role_name: "lambda-exec",
                path: None,
                policy: trust_policy("Allow", "\"sts:AssumeRole\"", "\"lambda.amazonaws.com\""),
            }),
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/lambda-exec"),
            this_account: true,
            assumable: false,
        },
        RoleRow {
            case: "a role whose statement denies the assume to SageMaker",
            fixture: Some(RoleFixture {
                role_name: "denied-exec",
                path: None,
                policy: trust_policy("Deny", "\"sts:AssumeRole\"", "\"sagemaker.amazonaws.com\""),
            }),
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/denied-exec"),
            this_account: true,
            assumable: false,
        },
        RoleRow {
            case: "a role whose statement allows SageMaker another action only",
            fixture: Some(RoleFixture {
                role_name: "tagsession-exec",
                path: None,
                policy: trust_policy("Allow", "\"sts:TagSession\"", "\"sagemaker.amazonaws.com\""),
            }),
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/tagsession-exec"),
            this_account: true,
            assumable: false,
        },
        RoleRow {
            case: "a role this account never created",
            fixture: None,
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/never-created-exec"),
            this_account: true,
            assumable: false,
        },
        RoleRow {
            // Another account's IAM is not this emulator's to ask, so nothing is learned
            // about the role and no create is refused for it.
            case: "a role of another account",
            fixture: None,
            role_arn: format!("arn:aws:iam::{OTHER_ACCOUNT}:role/sagemaker-exec"),
            this_account: false,
            assumable: true,
        },
    ]
}

/// How the AI services word a role SageMaker cannot assume: a fixed sentence.
fn role_cannot_be_assumed(_role_arn: &str) -> String {
    "Unable to assume the provided RoleArn. Ensure the role exists and its trust policy \
     allows this service to assume it."
        .to_string()
}

/// How the generic SageMaker create words it: a fixed sentence naming the action and the
/// principal.
fn unable_to_perform_assume_role(_role_arn: &str) -> String {
    "SageMaker is unable to perform: sts:AssumeRole on the role. Check if you have a trust \
     relationship for 'sagemaker.amazonaws.com' to assume the role."
        .to_string()
}

/// How HyperPod words it: the role ARN as the request carried it, then the fixed sentence.
fn hyperpod_cannot_assume(role_arn: &str) -> String {
    format!(
        "SageMaker HyperPod cannot assume the execution role {role_arn}. Please ensure that \
         the execution role allows the action sts:AssumeRole for the service principal \
         sagemaker.amazonaws.com."
    )
}

/// How the notebook-instance create words it: the role ARN as the request carried it, then
/// the fixed sentence naming the principal that has to be allowed to assume the role.
fn could_not_assume_role(role_arn: &str) -> String {
    format!(
        "Could not assume role {role_arn}. Please ensure that the role exists and allows \
         principal 'sagemaker.amazonaws.com' to assume the role."
    )
}

/// How the hosting creates word it: the role ARN as the request carried it, quoted.
fn execution_role_arn_is_invalid(role_arn: &str) -> String {
    format!(
        "The execution role ARN \"{role_arn}\" is invalid. Please ensure that the role exists \
         and that its trust relationship policy allows the action \"sts:AssumeRole\" for the \
         service principal \"sagemaker.amazonaws.com\"."
    )
}

/// How the create's answer is read back, which depends on how the service addresses the
/// resource.
enum Readback {
    /// The reader that reports the resource by the NAME the create gave it: it answers the
    /// resource when the create stored one, and a refusal when it did not.
    ByName {
        reader: &'static str,
        body: fn(&str) -> Value,
    },
    /// For a resource the service addresses by a server-minted id: the reader that reports
    /// it by the id the create answered under `member`, which a refused create mints none
    /// of.
    ByMintedId {
        reader: &'static str,
        member: &'static str,
    },
}

/// One create driven for every role row above.
struct RoleCreate {
    operation: &'static str,
    /// The request, given the resource name and the role ARN.
    body: fn(&str, &str) -> Value,
    /// The sentence this operation's family words an unassumable role with.
    refusal: fn(&str) -> String,
    /// Whether this operation refuses a role this account's IAM holds NOTHING under.
    refuses_an_absent_role: bool,
    readback: Readback,
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

/// A `HyperParameterTrainingJobDefinition` whose training jobs run as `role`.
fn training_job_definition(role: &str) -> Value {
    json!({
        "AlgorithmSpecification": {
            "TrainingImage": format!("{ACCOUNT}.dkr.ecr.us-east-1.amazonaws.com/xgboost:1"),
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
    })
}

/// A `HyperParameterTuningJobConfig` the model accepts, with no role of its own.
fn tuning_job_config() -> Value {
    json!({
        "Strategy": "Bayesian",
        "HyperParameterTuningJobObjective": {
            "Type": "Minimize",
            "MetricName": "validation:rmse",
        },
        "ResourceLimits": { "MaxNumberOfTrainingJobs": 4, "MaxParallelTrainingJobs": 2 },
        "TrainingJobEarlyStoppingType": "Off",
    })
}

/// The creates the role rule is driven over: one per resource family that words the refusal
/// its own way.
fn creates_under_a_role() -> Vec<RoleCreate> {
    vec![
        RoleCreate {
            operation: "CreateAIBenchmarkJob",
            body: |name, role| {
                json!({
                    "AIBenchmarkJobName": name,
                    "AIWorkloadConfigIdentifier": WORKLOAD_CONFIG,
                    "RoleArn": role,
                    "BenchmarkTarget": { "Endpoint": { "Identifier": BENCHMARK_ENDPOINT } },
                    "OutputConfig": { "S3OutputLocation": "s3://bucket/benchmarks/" },
                })
            },
            refusal: role_cannot_be_assumed,
            refuses_an_absent_role: true,
            readback: Readback::ByName {
                reader: "DescribeAIBenchmarkJob",
                body: |name| json!({ "AIBenchmarkJobName": name }),
            },
        },
        RoleCreate {
            operation: "CreateCluster",
            body: |name, role| {
                json!({
                    "ClusterName": name,
                    "InstanceGroups": [instance_group("workers", role)],
                })
            },
            refusal: hyperpod_cannot_assume,
            refuses_an_absent_role: true,
            readback: Readback::ByName {
                reader: "DescribeCluster",
                body: |name| json!({ "ClusterName": name }),
            },
        },
        RoleCreate {
            operation: "CreateHyperParameterTuningJob",
            body: |name, role| {
                json!({
                    "HyperParameterTuningJobName": name,
                    "HyperParameterTuningJobConfig": tuning_job_config(),
                    "TrainingJobDefinition": training_job_definition(role),
                })
            },
            refusal: unable_to_perform_assume_role,
            refuses_an_absent_role: true,
            readback: Readback::ByName {
                reader: "DescribeHyperParameterTuningJob",
                body: |name| json!({ "HyperParameterTuningJobName": name }),
            },
        },
        RoleCreate {
            operation: "CreateMlflowApp",
            body: |name, role| {
                json!({
                    "Name": name,
                    "ArtifactStoreUri": "s3://mlflow-artifacts/apps/",
                    "RoleArn": role,
                })
            },
            refusal: unable_to_perform_assume_role,
            refuses_an_absent_role: true,
            readback: Readback::ByMintedId {
                reader: "DescribeMlflowApp",
                member: "Arn",
            },
        },
        RoleCreate {
            // The hosting family is recorded registering a model around a role this
            // account's IAM holds nothing under, and refusing one whose stored trust
            // document admits another service only — so this create asks the reading that
            // refuses only the latter.
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
            refusal: execution_role_arn_is_invalid,
            refuses_an_absent_role: false,
            readback: Readback::ByName {
                reader: "DescribeModel",
                body: |name| json!({ "ModelName": name }),
            },
        },
        RoleCreate {
            // A notebook instance takes the same reading as the hosting creates: the
            // recorded service ACCEPTS one named against a role this account's IAM holds
            // nothing under and then drives the instance to the modelled `Failed` status
            // (asserted, with the status it settles into, by the SageMaker crate's
            // `model_artifact_admission` suite), while a role whose stored trust document
            // admits another service only is refused here.
            operation: "CreateNotebookInstance",
            body: |name, role| {
                json!({
                    "NotebookInstanceName": name,
                    "InstanceType": "ml.t3.medium",
                    "RoleArn": role,
                })
            },
            refusal: could_not_assume_role,
            refuses_an_absent_role: false,
            readback: Readback::ByName {
                reader: "DescribeNotebookInstance",
                body: |name| json!({ "NotebookInstanceName": name }),
            },
        },
    ]
}

/// Hold every role the table names in IAM, and seed the AI workload configuration and the
/// inference endpoint the benchmark job references.
fn seed(router: &smithy_server_runtime::MultiServiceRouter, runtime: &tokio::runtime::Runtime) {
    for row in role_rows() {
        let Some(fixture) = row.fixture else { continue };
        let mut parameters = vec![
            ("RoleName", fixture.role_name),
            ("AssumeRolePolicyDocument", fixture.policy.as_str()),
        ];
        if let Some(path) = fixture.path {
            parameters.push(("Path", path));
        }
        let (status, body) = iam(router, runtime, "CreateRole", &parameters);
        assert!(
            status == 200,
            "setup: CreateRole {}: expected 200, got {status} {body}",
            fixture.role_name
        );
    }
    let (status, body) = sagemaker(
        router,
        runtime,
        "CreateAIWorkloadConfig",
        &json!({ "AIWorkloadConfigName": WORKLOAD_CONFIG }),
    );
    assert!(
        status == 200,
        "setup: CreateAIWorkloadConfig: expected 200, got {status} {body}"
    );
    // `CreateAIBenchmarkJob` resolves its `BenchmarkTarget` against the endpoint store before
    // it writes anything, so the endpoint the row names is stood up here: what this suite
    // asserts is the ROLE rule, which a target the account does not hold would mask.
    let assumable_role = format!("arn:aws:iam::{ACCOUNT}:role/sagemaker-exec");
    for (operation, body) in [
        (
            "CreateModel",
            json!({
                "ModelName": format!("{BENCHMARK_ENDPOINT}-model"),
                "PrimaryContainer": {
                    "Image": format!("{ACCOUNT}.dkr.ecr.us-east-1.amazonaws.com/infer:1"),
                },
                "ExecutionRoleArn": assumable_role,
            }),
        ),
        (
            "CreateEndpointConfig",
            json!({
                "EndpointConfigName": format!("{BENCHMARK_ENDPOINT}-config"),
                "ProductionVariants": [{
                    "VariantName": "AllTraffic",
                    "ModelName": format!("{BENCHMARK_ENDPOINT}-model"),
                    "InstanceType": "ml.m5.large",
                    "InitialInstanceCount": 1,
                }],
            }),
        ),
        (
            "CreateEndpoint",
            json!({
                "EndpointName": BENCHMARK_ENDPOINT,
                "EndpointConfigName": format!("{BENCHMARK_ENDPOINT}-config"),
            }),
        ),
    ] {
        let (status, answered) = sagemaker(router, runtime, operation, &body);
        assert!(
            status == 200,
            "setup: {operation}: expected 200, got {status} {answered}"
        );
    }
}

/// RULE: a create whose resource runs as a caller-supplied role is accepted only where this
/// account's IAM answers that SageMaker may assume it, is refused 400 `ValidationException`
/// with its family's sentence otherwise, and stores nothing when refused.
#[test]
fn each_create_family_refuses_the_role_its_own_iam_answer_withholds() {
    std::thread::Builder::new()
        .name("sagemaker-execution-role-admission".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");
            seed(&router, &runtime);

            for (index, row) in role_rows().iter().enumerate() {
                for create in creates_under_a_role() {
                    let accepted = if create.refuses_an_absent_role {
                        row.accepted_when_absent_is_refused()
                    } else {
                        row.accepted_when_only_a_held_role_is_refused()
                    };
                    let name = format!("r{index}{}", create.operation.to_lowercase());
                    let case = format!("{} / {}", create.operation, row.case);
                    let (status, answered) = sagemaker(
                        &router,
                        &runtime,
                        create.operation,
                        &(create.body)(&name, &row.role_arn),
                    );

                    if accepted {
                        assert_eq!(
                            status, 200,
                            "case {case:?}: a role this IAM withholds nothing about must be \
                             accepted, got {status} {answered}"
                        );
                    } else {
                        assert_eq!(
                            (status, error_code(&answered).as_str()),
                            (400, "ValidationException"),
                            "case {case:?}: an unassumable role must be refused 400 \
                             ValidationException, got {status} {answered}"
                        );
                        assert_eq!(
                            error_message(&answered),
                            (create.refusal)(&row.role_arn),
                            "case {case:?}: expected this family's assume-role sentence, got \
                             {answered}"
                        );
                    }

                    // What the create did or did not store, read back through the service's
                    // own reader.
                    match create.readback {
                        Readback::ByName { reader, body } => {
                            let (read_status, read) =
                                sagemaker(&router, &runtime, reader, &body(&name));
                            if accepted {
                                assert_eq!(
                                    read_status, 200,
                                    "case {case:?}: the accepted create must store the \
                                     resource, got {read_status} {read}"
                                );
                            } else {
                                assert_eq!(
                                    read_status, 400,
                                    "case {case:?}: a refused create must store nothing, got \
                                     {read_status} {read}"
                                );
                            }
                        }
                        Readback::ByMintedId { reader, member } => {
                            let minted = answered.get(member).and_then(Value::as_str);
                            if !accepted {
                                assert!(
                                    minted.is_none(),
                                    "case {case:?}: a refused create must mint no {member}, \
                                     got {answered}"
                                );
                                continue;
                            }
                            let minted = minted.unwrap_or_else(|| {
                                panic!("case {case:?}: the create must answer {member}")
                            });
                            let (read_status, read) = sagemaker(
                                &router,
                                &runtime,
                                reader,
                                &json!({ member: minted }),
                            );
                            assert_eq!(
                                read_status, 200,
                                "case {case:?}: the accepted create must store the resource, \
                                 got {read_status} {read}"
                            );
                            assert_eq!(
                                read.get("RoleArn").and_then(Value::as_str),
                                Some(row.role_arn.as_str()),
                                "case {case:?}: the stored resource must carry the role it was \
                                 given, got {read}"
                            );
                        }
                    }
                }
            }
        })
        .expect("a sized thread")
        .join()
        .expect("the suite thread");
}

/// One request whose role travels in a member other than the create's own `RoleArn`, and
/// which role a refusal then names.
struct MultiRoleRequest {
    case: &'static str,
    /// The request, given the resource name, the assumable role and the unassumable one.
    body: fn(&str, &str, &str) -> Value,
    /// Which of the two roles the refusal has to name.
    refused_role_is_the_unassumable_one: bool,
}

/// RULE: a create that takes SEVERAL roles settles each of them, so the refusal names the
/// role the request itself declared — whichever member carries it — and the cluster the
/// request named is stored by neither.
#[test]
fn a_create_taking_several_roles_names_the_one_its_iam_answer_withholds() {
    std::thread::Builder::new()
        .name("sagemaker-execution-role-multi".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");
            seed(&router, &runtime);

            let good = format!("arn:aws:iam::{ACCOUNT}:role/sagemaker-exec");
            let bad = format!("arn:aws:iam::{ACCOUNT}:role/lambda-exec");

            let rows: Vec<MultiRoleRequest> = vec![
                MultiRoleRequest {
                    case: "the second instance group's execution role",
                    body: |name, good, bad| {
                        json!({
                            "ClusterName": name,
                            "InstanceGroups": [
                                instance_group("head", good),
                                instance_group("workers", bad),
                            ],
                        })
                    },
                    refused_role_is_the_unassumable_one: true,
                },
                MultiRoleRequest {
                    case: "the cluster's autoscaling role",
                    body: |name, good, bad| {
                        json!({
                            "ClusterName": name,
                            "InstanceGroups": [instance_group("head", good)],
                            "ClusterRole": bad,
                            "AutoScaling": { "Mode": "Enable", "AutoScalerType": "Karpenter" },
                        })
                    },
                    refused_role_is_the_unassumable_one: true,
                },
                MultiRoleRequest {
                    case: "every role assumable",
                    body: |name, good, _bad| {
                        json!({
                            "ClusterName": name,
                            "InstanceGroups": [
                                instance_group("head", good),
                                instance_group("workers", good),
                            ],
                            "ClusterRole": good,
                            "AutoScaling": { "Mode": "Enable", "AutoScalerType": "Karpenter" },
                        })
                    },
                    refused_role_is_the_unassumable_one: false,
                },
            ];

            for (index, row) in rows.iter().enumerate() {
                let name = format!("multirole{index}");
                let (status, answered) = sagemaker(
                    &router,
                    &runtime,
                    "CreateCluster",
                    &(row.body)(&name, &good, &bad),
                );
                if !row.refused_role_is_the_unassumable_one {
                    assert_eq!(
                        status, 200,
                        "case {:?}: a request whose every role is assumable must be accepted, \
                         got {status} {answered}",
                        row.case
                    );
                    continue;
                }
                assert_eq!(
                    (status, error_code(&answered).as_str()),
                    (400, "ValidationException"),
                    "case {:?}: expected 400 ValidationException, got {status} {answered}",
                    row.case
                );
                assert_eq!(
                    error_message(&answered),
                    hyperpod_cannot_assume(&bad),
                    "case {:?}: the refusal must name the role the request declared, got \
                     {answered}",
                    row.case
                );

                // The refused create stored no cluster, so the operations that follow it
                // report the name as NOT FOUND rather than as a cluster being created.
                for (reader, expected) in [
                    ("DescribeCluster", format!("Cluster with name {name} not found")),
                    ("DeleteCluster", format!("Cluster with name {name} not found")),
                ] {
                    let (read_status, read) = sagemaker(
                        &router,
                        &runtime,
                        reader,
                        &json!({ "ClusterName": name }),
                    );
                    assert_eq!(
                        (read_status, error_code(&read).as_str()),
                        (400, "ResourceNotFound"),
                        "case {:?}/{reader}: a refused create leaves the cluster unknown, got \
                         {read_status} {read}",
                        row.case
                    );
                    assert_eq!(
                        error_message(&read),
                        expected,
                        "case {:?}/{reader}: expected the not-found sentence, got {read}",
                        row.case
                    );
                }
            }
        })
        .expect("a sized thread")
        .join()
        .expect("the suite thread");
}

/// RULE: every operation that follows a create refused for its role reports the resource as
/// not found — the refused create stored none — with the sentence that resource words a
/// missing one in.
#[test]
fn the_operations_after_a_refused_create_report_the_resource_as_missing() {
    std::thread::Builder::new()
        .name("sagemaker-execution-role-followers".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");
            seed(&router, &runtime);

            // Rows differ along the role the request names: one IAM holds nothing under, and
            // one it holds for another service only.
            let rows = [
                ("a role this account never created", "never-created-exec"),
                ("a role that trusts another service only", "lambda-exec"),
            ];

            for (index, (case, role_name)) in rows.iter().enumerate() {
                let role = format!("arn:aws:iam::{ACCOUNT}:role/{role_name}");
                let name = format!("tuning{index}");
                let (status, answered) = sagemaker(
                    &router,
                    &runtime,
                    "CreateHyperParameterTuningJob",
                    &json!({
                        "HyperParameterTuningJobName": name,
                        "HyperParameterTuningJobConfig": tuning_job_config(),
                        "TrainingJobDefinition": training_job_definition(&role),
                    }),
                );
                assert_eq!(
                    (status, error_code(&answered).as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: expected 400 ValidationException, got {status} {answered}"
                );

                let expected =
                    format!("Amazon SageMaker can't find a tuning job called {name}. Type \
                             another name.");
                for reader in [
                    "DescribeHyperParameterTuningJob",
                    "StopHyperParameterTuningJob",
                ] {
                    let (read_status, read) = sagemaker(
                        &router,
                        &runtime,
                        reader,
                        &json!({ "HyperParameterTuningJobName": name }),
                    );
                    assert_eq!(
                        (read_status, error_code(&read).as_str()),
                        (400, "ResourceNotFound"),
                        "case {case:?}/{reader}: a refused create leaves the job unknown, got \
                         {read_status} {read}"
                    );
                    assert_eq!(
                        error_message(&read),
                        expected,
                        "case {case:?}/{reader}: expected the not-found sentence, got {read}"
                    );
                }
            }
        })
        .expect("a sized thread")
        .join()
        .expect("the suite thread");
}

/// RULE: `CreateImage` is accepted whatever its `RoleArn` names — the image is reported
/// `CREATING` straight after the call — and the status the clock then owes it is the
/// modelled `CREATE_FAILED` with the `FailureReason` naming the role when this account's
/// IAM answers that SageMaker cannot assume it, and `CREATED` with no `FailureReason`
/// otherwise.
#[test]
fn an_image_built_under_an_unassumable_role_settles_create_failed() {
    std::thread::Builder::new()
        .name("sagemaker-execution-role-image".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");
            seed(&router, &runtime);

            for (index, row) in role_rows().iter().enumerate() {
                let name = format!("image{index}");
                let case = row.case;
                let (status, created) = sagemaker(
                    &router,
                    &runtime,
                    "CreateImage",
                    &json!({ "ImageName": name, "RoleArn": row.role_arn }),
                );
                assert_eq!(
                    status, 200,
                    "case {case:?}: CreateImage must be accepted whatever the role, got {status} \
                     {created}"
                );

                // Before the owed instant the image is still being created, whatever the
                // answer about its role was.
                let (status, creating) = sagemaker(
                    &router,
                    &runtime,
                    "DescribeImage",
                    &json!({ "ImageName": name }),
                );
                assert_eq!(
                    (status, creating.get("ImageStatus").and_then(Value::as_str)),
                    (200, Some("CREATING")),
                    "case {case:?}: a freshly created image must be reported CREATING, got \
                     {status} {creating}"
                );

                std::thread::sleep(PAST_IMAGE_SETTLE_DUE);
                let (status, settled) = sagemaker(
                    &router,
                    &runtime,
                    "DescribeImage",
                    &json!({ "ImageName": name }),
                );
                assert_eq!(
                    status, 200,
                    "case {case:?}: DescribeImage must answer the stored image, got {status} \
                     {settled}"
                );
                let assumable = row.accepted_when_absent_is_refused();
                let expected_status = if assumable { "CREATED" } else { "CREATE_FAILED" };
                assert_eq!(
                    settled.get("ImageStatus").and_then(Value::as_str),
                    Some(expected_status),
                    "case {case:?}: expected the image to settle {expected_status}, got {settled}"
                );
                let expected_reason = (!assumable)
                    .then(|| format!("Unable to assume role with RoleArn {}", row.role_arn));
                assert_eq!(
                    settled
                        .get("FailureReason")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    expected_reason,
                    "case {case:?}: expected the failure reason to name the role, got {settled}"
                );

                // The settled status is stable: a second read does not move it.
                let (status, again) = sagemaker(
                    &router,
                    &runtime,
                    "DescribeImage",
                    &json!({ "ImageName": name }),
                );
                assert_eq!(
                    (status, again.get("ImageStatus").and_then(Value::as_str)),
                    (200, Some(expected_status)),
                    "case {case:?}: a later read must report the same status, got {status} {again}"
                );
            }
        })
        .expect("a sized thread")
        .join()
        .expect("the suite thread");
}
