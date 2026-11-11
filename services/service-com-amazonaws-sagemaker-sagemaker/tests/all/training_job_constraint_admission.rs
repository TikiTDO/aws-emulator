// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for what `CreateTrainingJob` ADMITS and what it REFUSES on the
//! strength of the constraints the model declares for its members.
//!
//! The admission and replay of a training job, and the way an `AlgorithmSpecification`
//! must name the algorithm exactly once, are pinned by
//! [`crate::training_job_admission`]; this suite pins the other half of the same
//! operation's admission: a member value is admitted when it is INSIDE the constraint its
//! target shape declares and refused when it is outside, with the 400
//! `ValidationException` envelope naming the member path and the requirement it failed —
//! and with nothing stored for a refused request.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, reusing
//! the harness in [`crate::inference_deployment_lifecycle`], so what these tests exercise
//! is what a real SDK client gets.
//!
//! Each test asserts the rule over a TABLE of inputs: the ARN forms a pattern-constrained
//! member accepts and rejects, and for every numeric range the operation carries, each
//! PUBLISHED bound and one value past it.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The `ValidationException` envelope around exactly one clause, as the framework writes
/// it (the form `rejection_fidelity` pins for this service).
fn one_violation(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// A `CreateTrainingJob` body naming `name`, carrying the members the model marks required
/// plus the `ResourceConfig` and `StoppingCondition` the rows below vary
/// (`smithy_query(query="operation",operation="CreateTrainingJob")`).
fn training_body(name: &str) -> Value {
    json!({
        "TrainingJobName": name,
        "AlgorithmSpecification": {
            "TrainingImage": format!("{ACCOUNT}.dkr.ecr.us-east-1.amazonaws.com/trainer:1"),
            "TrainingInputMode": "File",
        },
        "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/service-role/SageMakerTrainingRole"),
        "InputDataConfig": [{
            "ChannelName": "train",
            "DataSource": {
                "S3DataSource": { "S3DataType": "S3Prefix", "S3Uri": "s3://demo/train/" },
            },
        }],
        "OutputDataConfig": { "S3OutputPath": "s3://demo/output/" },
        "ResourceConfig": {
            "InstanceType": "ml.m5.large",
            "InstanceCount": 1,
            "VolumeSizeInGB": 10,
        },
        "StoppingCondition": { "MaxRuntimeInSeconds": 3600 },
    })
}

/// Whether the account holds a training job under `name`, asked through the implemented
/// `DescribeTrainingJob` — so a refused create is shown to have stored nothing and an
/// admitted one to have stored the job.
async fn is_stored(emulator: &Emulator, name: &str) -> bool {
    let (status, _) = emulator
        .call("DescribeTrainingJob", &json!({ "TrainingJobName": name }))
        .await;
    status == 200
}

/// RULE: a member whose target shape declares a `@pattern` is admitted for every value
/// that satisfies the expression and refused for every value that does not, with the
/// unanchored expression stated as the requirement — so what the service enforces is the
/// MODEL's expression and not some other idea of the identifier's shape.
///
/// The member here is `CreateTrainingJob$RoleArn`
/// (`smithy_query(query="shape",shape="RoleArn")`: `len 20..2048`, pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`), the one ARN the request
/// carries. Rows differ along the partition, the presence of a role path, the characters
/// the path carries, the separator after `role`, the resource type the ARN names and the
/// width of the account id — every refused row is at least the 20 characters the length
/// constraint asks for, so the one clause it fails is the pattern.
#[tokio::test]
async fn a_role_arn_is_judged_against_the_modelled_pattern() {
    // The expression as the service reports it: the model's, without the anchors.
    let reported = r"arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+";
    let requirement = format!("Member must satisfy regular expression pattern: {reported}");

    // (case, the RoleArn, whether the pattern admits it)
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "a role with no path",
            format!("arn:aws:iam::{ACCOUNT}:role/SageMakerExecution"),
            true,
        ),
        (
            "a role under a service-role path",
            format!("arn:aws:iam::{ACCOUNT}:role/service-role/SageMakerTrainingRole"),
            true,
        ),
        (
            "every character class the expression allows in the path",
            format!("arn:aws:iam::{ACCOUNT}:role/team=science,group+1/Train_Role.v2@x-1"),
            true,
        ),
        (
            // `aws[a-z\-]*` is the partition, so a non-commercial one satisfies it.
            "a role in another partition",
            format!("arn:aws-cn:iam::{ACCOUNT}:role/PartitionRole"),
            true,
        ),
        (
            // `role/?` makes the separator optional, so the expression admits this even
            // though no IAM console writes an ARN this way.
            "no separator after role",
            format!("arn:aws:iam::{ACCOUNT}:roleSageMakerExecution"),
            true,
        ),
        ("no ARN at all", "not-an-iam-role-arn-value".to_string(), false),
        (
            "an ARN of another service",
            "arn:aws:s3:::sagemaker-training-bucket".to_string(),
            false,
        ),
        (
            "an IAM ARN naming a user rather than a role",
            format!("arn:aws:iam::{ACCOUNT}:user/not-a-role"),
            false,
        ),
        (
            "an account id narrower than twelve digits",
            "arn:aws:iam::12345:role/ShortAccountId".to_string(),
            false,
        ),
        (
            "a region where an IAM ARN carries none",
            format!("arn:aws:iam:us-east-1:{ACCOUNT}:role/RegionalRole"),
            false,
        ),
        (
            // `/` is itself one of the characters the trailing class admits, so the
            // expression reads this as `role` followed by a one-character name: inside
            // the pattern, however little a role it looks like.
            "the separator with no name after it",
            format!("arn:aws:iam::{ACCOUNT}:role/"),
            true,
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, role_arn, admitted)) in rows.into_iter().enumerate() {
        let name = format!("role-case-{index}");
        let mut body = training_body(&name);
        body["RoleArn"] = json!(role_arn);
        let (status, answer) = emulator.call("CreateTrainingJob", &body).await;

        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: a RoleArn the pattern admits must be accepted, got {status} \
                 with {answer}"
            );
            let described = emulator
                .ok("DescribeTrainingJob", &json!({ "TrainingJobName": name }))
                .await;
            assert_eq!(
                str_field(&described, "RoleArn"),
                role_arn,
                "case {case:?}: the admitted role must be replayed as stored, got {described}"
            );
            continue;
        }

        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {answer}"
        );
        assert_eq!(
            message(&answer),
            one_violation(&format!(
                "Value '{role_arn}' at 'roleArn' failed to satisfy constraint: {requirement}"
            )),
            "case {case:?}: the refusal's message, got {answer}"
        );
        assert!(
            !is_stored(&emulator, &name).await,
            "case {case:?}: a refused create must store no training job"
        );
    }
}

/// RULE: a numeric member is admitted at every value inside the `@range` its target shape
/// declares — the published bounds included — and refused outside it, with the envelope
/// echoing the value, naming the member path and stating the bound it failed; a refused
/// request stores nothing.
///
/// The ranges are the ones `CreateTrainingJob`'s members carry
/// (`smithy_query(query="shape",shape="ResourceConfig,StoppingCondition")`):
/// `ResourceConfig$InstanceCount` -> `TrainingInstanceCount` `range {min 0}`,
/// `ResourceConfig$VolumeSizeInGB` `range {min 0}`,
/// `ResourceConfig$KeepAlivePeriodInSeconds` `range {0, 21600}`,
/// `StoppingCondition$MaxRuntimeInSeconds` `range {min 1}` and
/// `StoppingCondition$MaxPendingTimeInSeconds` `range {1800, 2419200}`. Each row is a
/// published bound or one value past it. An instance count of zero is inside the modelled
/// range and is refused by a rule of the SERVICE instead — a job runs on at least one
/// instance — which [`crate::admission_bounds`] pins over that whole class, so it is not a
/// row here.
#[tokio::test]
async fn a_numeric_member_is_judged_against_each_published_bound_of_its_range() {
    // (case, the request member to set, the wire path the envelope names, the value, the
    // bound clause the refusal states — `None` where the range admits the value)
    let rows: Vec<(&str, &str, &str, i64, Option<&str>)> = vec![
        (
            "an instance count of one",
            "InstanceCount",
            "resourceConfig.instanceCount",
            1,
            None,
        ),
        (
            "a distributed instance count",
            "InstanceCount",
            "resourceConfig.instanceCount",
            8,
            None,
        ),
        (
            "an instance count one below the published minimum",
            "InstanceCount",
            "resourceConfig.instanceCount",
            -1,
            Some("greater than or equal to 0"),
        ),
        (
            "an instance count far below the published minimum",
            "InstanceCount",
            "resourceConfig.instanceCount",
            -1_000,
            Some("greater than or equal to 0"),
        ),
        (
            "a volume size at its published minimum",
            "VolumeSizeInGB",
            "resourceConfig.volumeSizeInGB",
            0,
            None,
        ),
        (
            "a volume size of one",
            "VolumeSizeInGB",
            "resourceConfig.volumeSizeInGB",
            1,
            None,
        ),
        (
            "a volume size one below its published minimum",
            "VolumeSizeInGB",
            "resourceConfig.volumeSizeInGB",
            -1,
            Some("greater than or equal to 0"),
        ),
        (
            "a warm-pool retention at its published minimum",
            "KeepAlivePeriodInSeconds",
            "resourceConfig.keepAlivePeriodInSeconds",
            0,
            None,
        ),
        (
            "a warm-pool retention at its published maximum",
            "KeepAlivePeriodInSeconds",
            "resourceConfig.keepAlivePeriodInSeconds",
            21_600,
            None,
        ),
        (
            "a warm-pool retention one past its published maximum",
            "KeepAlivePeriodInSeconds",
            "resourceConfig.keepAlivePeriodInSeconds",
            21_601,
            Some("less than or equal to 21600"),
        ),
        (
            "a maximum runtime at its published minimum",
            "MaxRuntimeInSeconds",
            "stoppingCondition.maxRuntimeInSeconds",
            1,
            None,
        ),
        (
            "a maximum runtime one below its published minimum",
            "MaxRuntimeInSeconds",
            "stoppingCondition.maxRuntimeInSeconds",
            0,
            Some("greater than or equal to 1"),
        ),
        (
            "a maximum pending time at its published minimum",
            "MaxPendingTimeInSeconds",
            "stoppingCondition.maxPendingTimeInSeconds",
            1_800,
            None,
        ),
        (
            "a maximum pending time one below its published minimum",
            "MaxPendingTimeInSeconds",
            "stoppingCondition.maxPendingTimeInSeconds",
            1_799,
            Some("greater than or equal to 1800"),
        ),
        (
            "a maximum pending time at its published maximum",
            "MaxPendingTimeInSeconds",
            "stoppingCondition.maxPendingTimeInSeconds",
            2_419_200,
            None,
        ),
        (
            "a maximum pending time one past its published maximum",
            "MaxPendingTimeInSeconds",
            "stoppingCondition.maxPendingTimeInSeconds",
            2_419_201,
            Some("less than or equal to 2419200"),
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, member, path, value, bound)) in rows.into_iter().enumerate() {
        let name = format!("range-case-{index}");
        let mut body = training_body(&name);
        // The two structures the rows vary sit at `ResourceConfig` and
        // `StoppingCondition`; the member decides which one carries the value.
        let holder = if path.starts_with("stoppingCondition") {
            "StoppingCondition"
        } else {
            "ResourceConfig"
        };
        body[holder][member] = json!(value);
        let (status, answer) = emulator.call("CreateTrainingJob", &body).await;

        let Some(bound) = bound else {
            assert_eq!(
                status, 200,
                "case {case:?}: a value inside the modelled range must be accepted, got \
                 {status} with {answer}"
            );
            let described = emulator
                .ok("DescribeTrainingJob", &json!({ "TrainingJobName": name }))
                .await;
            assert_eq!(
                described.get(holder).and_then(|held| held.get(member)),
                Some(&json!(value)),
                "case {case:?}: the admitted {member} must be replayed as stored, got \
                 {described}"
            );
            continue;
        };

        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {answer}"
        );
        assert_eq!(
            message(&answer),
            one_violation(&format!(
                "Value '{value}' at '{path}' failed to satisfy constraint: Member must have \
                 value {bound}"
            )),
            "case {case:?}: the refusal's message, got {answer}"
        );
        assert!(
            !is_stored(&emulator, &name).await,
            "case {case:?}: a refused create must store no training job"
        );
    }
}
