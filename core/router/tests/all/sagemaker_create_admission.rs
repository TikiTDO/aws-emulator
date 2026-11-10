// SPDX-License-Identifier: CC0-1.0

//! SageMaker resolves the IAM role a create's resource would RUN AS before it stores
//! anything, and resolves nothing else a create only NAMES.
//!
//! The rules under test:
//!
//! * a create whose resource RUNS AS a caller-supplied IAM role is refused HTTP 400
//!   `ValidationException` — persisting nothing — unless this account holds that role and
//!   its trust policy admits the SageMaker service principal; a role that exists and does
//!   trust SageMaker is accepted normally;
//! * a model package is registered around the container image path the request names
//!   whatever the registry holds, which is what the recorded service does.
//!
//! The first rule reaches ANOTHER service through the in-process callback, so a
//! single-service router has nothing to ask and this suite needs three services on one
//! engine — which is why it lives here rather than in the SageMaker crate (whose
//! `tests/all/create_admission.rs` covers the admission rules that are decided inside the
//! service).
//!
//! The rows span what each rule generalises over: for the role, the operation that takes it
//! (three creates, each of a different resource kind), the trust policy's shape, where the
//! role lives (the default path and a path), which account the ARN names, and whether the
//! role was ever created; for the image, whether the registry holds the repository, whether
//! the repository holds the tag, and the tag-versus-digest form of the path.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
pub(crate) const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// The account the emulator serves, which is the only registry and the only IAM it holds.
pub(crate) const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// A syntactically valid account id that is NOT this one.
pub(crate) const OTHER_ACCOUNT: &str = "123456789012";

/// The `X-Amz-Target` prefix ECR routes on.
const ECR_TARGET_PREFIX: &str = "AmazonEC2ContainerRegistry_V20150921";

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The
/// signature is never checked: this router runs with authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// One request through the assembled router.
fn send(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    target: Option<&str>,
    content_type: &str,
    body: String,
) -> (u16, String) {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com/"))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", content_type);
    if let Some(target) = target {
        builder = builder.header("x-amz-target", target);
    }
    let request = builder
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Drive one awsJson1_1 SageMaker call.
pub(crate) fn sagemaker(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let (status, text) = send(
        router,
        runtime,
        "sagemaker",
        Some(&format!("SageMaker.{operation}")),
        "application/x-amz-json-1.1",
        body.to_string(),
    );
    (status, parsed(&text))
}

/// Drive one awsJson1_1 Amazon ECR call.
fn ecr(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let (status, text) = send(
        router,
        runtime,
        "ecr",
        Some(&format!("{ECR_TARGET_PREFIX}.{operation}")),
        "application/x-amz-json-1.1",
        body.to_string(),
    );
    (status, parsed(&text))
}

/// Drive one awsQuery IAM action: `POST /` with the parameters in the form body. Every
/// value here is free of `&`, `=`, `+` and `%`, so it needs no escaping.
pub(crate) fn iam(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    parameters: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (key, value) in parameters {
        body.push_str(&format!("&{key}={value}"));
    }
    send(
        router,
        runtime,
        "iam",
        None,
        "application/x-www-form-urlencoded",
        body,
    )
}

/// A response body as JSON, or `Null` for an empty one.
fn parsed(text: &str) -> Value {
    if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(text).unwrap_or(Value::Null)
    }
}

/// The awsJson error code, without its namespace.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("")
        .to_string()
}

/// The error message, whichever casing the envelope carries it under.
pub(crate) fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// A trust policy with one statement, built from the pieces that vary between rows.
///
/// Written compactly and with no `&`, `=`, `+` or `%`, so it travels in an `awsQuery` form
/// body as written.
pub(crate) fn trust_policy(effect: &str, action: &str, service: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"Service\":{service}}},\"Action\":{action}}}]}}"
    )
}

/// One IAM role a suite needs, as `CreateRole` parameters.
///
/// Shared with the sibling SageMaker suites (`sagemaker_pipeline_role_resolution`), so a
/// role fixture is declared and held one way across them.
pub(crate) struct RoleFixture {
    pub(crate) role_name: &'static str,
    pub(crate) path: Option<&'static str>,
    pub(crate) policy: String,
}

/// Hold `fixture` in this account's IAM, failing the suite if IAM refuses it.
pub(crate) fn create_role(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    fixture: &RoleFixture,
) {
    let mut parameters = vec![
        ("RoleName", fixture.role_name),
        ("AssumeRolePolicyDocument", fixture.policy.as_str()),
    ];
    if let Some(path) = fixture.path {
        parameters.push(("Path", path));
    }
    let (status, body) = iam(router, runtime, "CreateRole", &parameters);
    assert_eq!(
        status, 200,
        "setup: CreateRole {}: got {status} {body}",
        fixture.role_name
    );
}

/// The roles the rows below name: trust policies differing in the shape AWS accepts for
/// `Action` and `Principal.Service` (one string, or a list), in their effect, and in the
/// principal they name.
fn role_fixtures() -> Vec<RoleFixture> {
    vec![
        RoleFixture {
            role_name: "sagemaker-role",
            path: None,
            policy: trust_policy("Allow", "\"sts:AssumeRole\"", "\"sagemaker.amazonaws.com\""),
        },
        RoleFixture {
            role_name: "sagemaker-role-lists",
            path: None,
            policy: trust_policy(
                "Allow",
                "[\"sts:TagSession\",\"sts:AssumeRole\"]",
                "[\"glue.amazonaws.com\",\"sagemaker.amazonaws.com\"]",
            ),
        },
        RoleFixture {
            role_name: "path-scoped-role",
            path: Some("/service-role/"),
            policy: trust_policy("Allow", "\"sts:AssumeRole\"", "\"sagemaker.amazonaws.com\""),
        },
        RoleFixture {
            role_name: "lambda-only-role",
            path: None,
            policy: trust_policy("Allow", "\"sts:AssumeRole\"", "\"lambda.amazonaws.com\""),
        },
        RoleFixture {
            role_name: "denied-role",
            path: None,
            policy: trust_policy("Deny", "\"sts:AssumeRole\"", "\"sagemaker.amazonaws.com\""),
        },
    ]
}

/// One role the creates are driven with, and whether SageMaker may assume it.
struct RoleCase {
    case: &'static str,
    role_arn: String,
    accepted: bool,
}

/// The role values the rows below drive every create with.
fn role_cases() -> Vec<RoleCase> {
    vec![
        RoleCase {
            case: "a role that trusts SageMaker",
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/sagemaker-role"),
            accepted: true,
        },
        RoleCase {
            case: "a role whose trust policy states its action and principal as lists",
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/sagemaker-role-lists"),
            accepted: true,
        },
        RoleCase {
            case: "a role under a path",
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/service-role/path-scoped-role"),
            accepted: true,
        },
        RoleCase {
            case: "a role that trusts another service only",
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/lambda-only-role"),
            accepted: false,
        },
        RoleCase {
            case: "a role whose statement denies the assume",
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/denied-role"),
            accepted: false,
        },
        RoleCase {
            case: "a role this account never created",
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/never-created"),
            accepted: false,
        },
        RoleCase {
            // Another account's IAM is not this emulator's to ask, so nothing is
            // learned about the role and the create is not refused for it.
            case: "a role of another account",
            role_arn: format!("arn:aws:iam::{OTHER_ACCOUNT}:role/sagemaker-role"),
            accepted: true,
        },
    ]
}

/// How SageMaker's JOB family words a role it cannot assume. The families that word the
/// same fact differently — the AI services, HyperPod, hosting, and the generic
/// sts:AssumeRole sentence — are driven by `sagemaker_execution_role_admission` instead,
/// which also covers the hosting reading: the behavioural corpus records `CreateModel`
/// ACCEPTING a role this account's IAM holds nothing under, so that operation refuses only
/// what a STORED trust document withholds.
fn could_not_assume(role_arn: &str) -> String {
    format!(
        "Could not assume role {role_arn}. Please ensure that the role exists and allows \
         principal 'sagemaker.amazonaws.com' to assume the role."
    )
}

/// One create driven for every role above, with the reader that proves nothing was stored.
struct CreateUnderARole {
    /// The create operation.
    operation: &'static str,
    /// The request body, given the resource name and the role ARN.
    body: fn(&str, &str) -> Value,
    /// The sentence this operation's family words an unassumable role with.
    refusal: fn(&str) -> String,
    /// The reader that reports the resource, and its request body given the name.
    reader: &'static str,
    read_body: fn(&str) -> Value,
}

/// A container image path in this account's registry.
fn image_uri(repository: &str, tag: &str) -> String {
    format!("{ACCOUNT}.dkr.ecr.{REGION}.amazonaws.com/{repository}:{tag}")
}

/// The creates the role rule is driven over, one per resource kind.
///
/// `CreateNotebookInstance` is NOT among them: the hosting reading it takes — accept a role
/// this account's IAM holds nothing under, and fail the resource instead — is driven by
/// `sagemaker_execution_role_admission` beside the other creates that take it. Neither is
/// `CreatePipeline`: registering a pipeline assumes nothing, so it resolves no role at all
/// and the resolution belongs to the run that performs a step as it — driven by
/// `sagemaker_pipeline_role_resolution`.
fn creates_under_a_role() -> Vec<CreateUnderARole> {
    vec![
        CreateUnderARole {
            operation: "CreateProcessingJob",
            body: |name, role| {
                json!({
                    "ProcessingJobName": name,
                    "RoleArn": role,
                    "AppSpecification": { "ImageUri": image_uri("proc", "1") },
                    "ProcessingResources": {
                        "ClusterConfig": {
                            "InstanceCount": 1,
                            "InstanceType": "ml.m5.large",
                            "VolumeSizeInGB": 30,
                        },
                    },
                })
            },
            reader: "DescribeProcessingJob",
            read_body: |name| json!({ "ProcessingJobName": name }),
            refusal: could_not_assume,
        },
        CreateUnderARole {
            operation: "CreateTrainingJob",
            body: |name, role| {
                json!({
                    "TrainingJobName": name,
                    "RoleArn": role,
                    "AlgorithmSpecification": {
                        "TrainingImage": image_uri("trainer", "1"),
                        "TrainingInputMode": "File",
                    },
                    "OutputDataConfig": { "S3OutputPath": "s3://artifacts/out" },
                    "ResourceConfig": {
                        "InstanceType": "ml.m5.large",
                        "InstanceCount": 1,
                        "VolumeSizeInGB": 30,
                    },
                    "StoppingCondition": { "MaxRuntimeInSeconds": 3600 },
                })
            },
            reader: "DescribeTrainingJob",
            read_body: |name| json!({ "TrainingJobName": name }),
            refusal: could_not_assume,
        },
        CreateUnderARole {
            operation: "CreateJob",
            body: |name, role| {
                json!({
                    "JobName": name,
                    "JobCategory": "AgentRFT",
                    "JobConfigSchemaVersion": "1.0.0",
                    "JobConfigDocument": json!({
                        "AgentConfig": {},
                        "InputDataConfig": {},
                        "ModelPackageConfig": {},
                        "OutputDataConfig": {},
                        "TrainingConfig": {},
                    })
                    .to_string(),
                    "RoleArn": role,
                })
            },
            reader: "DescribeJob",
            read_body: |name| json!({ "JobName": name, "JobCategory": "AgentRFT" }),
            refusal: could_not_assume,
        },
    ]
}

#[test]
fn a_sagemaker_create_is_accepted_only_for_a_role_iam_holds_and_sagemaker_may_assume() {
    std::thread::Builder::new()
        .name("sagemaker-create-admission-role".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            for fixture in role_fixtures() {
                create_role(&router, &runtime, &fixture);
            }

            for (index, role) in role_cases().iter().enumerate() {
                for create in creates_under_a_role() {
                    let name = format!("admission{index}{}", create.operation.to_lowercase());
                    let request = (create.body)(&name, &role.role_arn);
                    let (status, answered) =
                        sagemaker(&router, &runtime, create.operation, &request);
                    let read = sagemaker(
                        &router,
                        &runtime,
                        create.reader,
                        &(create.read_body)(&name),
                    );
                    let case = format!("{} / {}", create.operation, role.case);

                    if role.accepted {
                        assert_eq!(
                            status, 200,
                            "case {case:?}: an assumable role must be accepted, got {status} \
                             {answered}"
                        );
                        assert_eq!(
                            read.0, 200,
                            "case {case:?}: the resource must be stored, got {} {}",
                            read.0, read.1
                        );
                        continue;
                    }
                    assert_eq!(
                        (status, error_code(&answered).as_str()),
                        (400, "ValidationException"),
                        "case {case:?}: an unassumable role must be refused 400 \
                         ValidationException, got {status} {answered}"
                    );
                    assert_eq!(
                        error_message(&answered),
                        (create.refusal)(&role.role_arn),
                        "case {case:?}: expected the assume-role refusal, got {answered}"
                    );
                    assert_eq!(
                        read.0, 400,
                        "case {case:?}: a refused create must store nothing, got {} {}",
                        read.0, read.1
                    );
                }
            }
        })
        .expect("a sized thread")
        .join()
        .expect("the suite thread");
}

/// One container image a model package is registered around.
struct ImageCase {
    case: &'static str,
    image: String,
}

/// RULE: a model package is registered around the container image path the request names,
/// WHATEVER the registry holds — the recorded service registers a package naming an image in
/// a registry of another account, and one naming a repository this account's registry does
/// not hold, so registration resolves no image. What it does hold the path to is its FORM:
/// one of the two the member documents, so a value naming no registry is refused (the rows
/// for that live in the SageMaker crate's `model_package_admission` suite).
#[test]
fn a_model_package_is_registered_around_whatever_image_the_request_names() {
    std::thread::Builder::new()
        .name("sagemaker-create-admission-image".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // The registry holds ONE repository with ONE tagged image; every other path
            // below names something it does not hold, and is registered just the same.
            let (status, created) = ecr(
                &router,
                &runtime,
                "CreateRepository",
                &json!({ "repositoryName": "inference" }),
            );
            assert_eq!(
                status, 200,
                "setup: CreateRepository must succeed, got {status} {created}"
            );
            let (status, pushed) = ecr(
                &router,
                &runtime,
                "PutImage",
                &json!({
                    "repositoryName": "inference",
                    "imageManifest": "{\"schemaVersion\":2}",
                    "imageTag": "1",
                }),
            );
            assert_eq!(
                status, 200,
                "setup: PutImage must succeed, got {status} {pushed}"
            );
            let digest = pushed
                .pointer("/image/imageId/imageDigest")
                .and_then(Value::as_str)
                .expect("the pushed image reports its digest")
                .to_string();

            // A role SageMaker may assume, so nothing but the IMAGE varies below.
            let policy = trust_policy("Allow", "\"sts:AssumeRole\"", "\"sagemaker.amazonaws.com\"");
            let (status, body) = iam(
                &router,
                &runtime,
                "CreateRole",
                &[
                    ("RoleName", "sagemaker-role"),
                    ("AssumeRolePolicyDocument", policy.as_str()),
                ],
            );
            assert_eq!(status, 200, "setup: CreateRole: got {status} {body}");

            let cases = [
                ImageCase {
                    case: "the tag the repository holds",
                    image: image_uri("inference", "1"),
                },
                ImageCase {
                    case: "the digest the repository holds",
                    image: format!("{ACCOUNT}.dkr.ecr.{REGION}.amazonaws.com/inference@{digest}"),
                },
                ImageCase {
                    case: "a tag the repository does not hold",
                    image: image_uri("inference", "99"),
                },
                ImageCase {
                    case: "a repository the registry does not hold",
                    image: image_uri("absent-repository", "1"),
                },
                ImageCase {
                    case: "a namespaced repository the registry does not hold",
                    image: image_uri("team/absent", "latest"),
                },
                ImageCase {
                    case: "another account's registry",
                    image: format!("{OTHER_ACCOUNT}.dkr.ecr.{REGION}.amazonaws.com/inference:1"),
                },
                ImageCase {
                    case: "another registry's host",
                    image: "registry.example.com/team/inference:1".to_string(),
                },
            ];

            for (index, case) in cases.iter().enumerate() {
                let name = format!("package{index}");
                let request = json!({
                    "ModelPackageName": name,
                    "InferenceSpecification": {
                        "Containers": [{ "Image": case.image }],
                        "SupportedContentTypes": ["application/json"],
                        "SupportedResponseMIMETypes": ["application/json"],
                    },
                });
                let (status, answered) =
                    sagemaker(&router, &runtime, "CreateModelPackage", &request);
                let read = sagemaker(
                    &router,
                    &runtime,
                    "DescribeModelPackage",
                    &json!({ "ModelPackageName": name }),
                );

                assert_eq!(
                    status, 200,
                    "case {:?}: expected the registration to be accepted, got {status} \
                     {answered}",
                    case.case
                );
                assert_eq!(
                    read.0, 200,
                    "case {:?}: the package must be stored, got {} {}",
                    case.case, read.0, read.1
                );
                assert_eq!(
                    read.1
                        .pointer("/InferenceSpecification/Containers/0/Image")
                        .and_then(Value::as_str),
                    Some(case.image.as_str()),
                    "case {:?}: the stored package must carry the image it was given, got {}",
                    case.case,
                    read.1
                );
            }
        })
        .expect("a sized thread")
        .join()
        .expect("the suite thread");
}
