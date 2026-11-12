// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's training jobs: what `CreateTrainingJob` admits,
//! how it refuses an `AlgorithmSpecification` that does not name the training algorithm
//! exactly once, and what `DescribeTrainingJob` replays.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, reusing
//! the harness in [`crate::inference_deployment_lifecycle`], so what these tests exercise
//! is what a real SDK client gets.
//!
//! The tests assert RULES over TABLES of inputs rather than one recorded call:
//!
//! * an accepted job is stored and replayed — its ARN, its `InProgress` status with the
//!   secondary status of a job whose work has not begun, the artifact path derived from
//!   the output location, and every request member the describe reports — for several
//!   shapes of request;
//! * the terminal status the clock owes carries the reason no container ran, never a
//!   `Completed` nothing earned;
//! * an `AlgorithmSpecification` names the algorithm exactly once, and the `AlgorithmName`
//!   reference is RESOLVED FIRST, so a name no algorithm is stored under is refused for
//!   that whatever else the spec carries;
//! * every algorithm reference this service resolves — the readers, the delete and the
//!   training-job create — words an algorithm the account does not hold the same way.

use std::time::Duration;

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// Long enough to pass `state::TRAINING_JOB_ENDS_AFTER`, whatever that span is.
const PAST_ENDS_DUE: Duration =
    service_com_amazonaws_sagemaker_sagemaker::state::TRAINING_JOB_ENDS_AFTER
        .saturating_mul(2)
        .saturating_add(Duration::from_millis(100));

/// The region every request below signs for, so a minted ARN is comparable to one the
/// test builds.
const REGION: &str = "us-east-1";

/// An IAM role ARN matching `RoleArn`'s modelled pattern.
fn role() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/service-role/SageMakerTrainingRole")
}

/// A training image URI, as `AlgorithmSpecification$TrainingImage` carries one.
fn training_image() -> String {
    format!("{ACCOUNT}.dkr.ecr.us-east-1.amazonaws.com/trainer:1")
}

/// An `arn:aws:sagemaker:<region>:<account>:<kind>/<name>` ARN, as this service mints one.
fn arn(kind: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:{kind}/{name}")
}

/// How SageMaker words an algorithm the account does not hold, for the algorithm `name`
/// resolves to.
fn algorithm_missing(name: &str) -> String {
    format!("Algorithm {} does not exist.", arn("algorithm", name))
}

/// A `CreateTrainingJob` body naming `name`, with `specification` as its
/// `AlgorithmSpecification` and `output` as its output location.
pub(crate) fn training_body(name: &str, specification: Value, output: &str) -> Value {
    json!({
        "TrainingJobName": name,
        "AlgorithmSpecification": specification,
        "RoleArn": role(),
        "InputDataConfig": [{
            "ChannelName": "train",
            "DataSource": {
                "S3DataSource": { "S3DataType": "S3Prefix", "S3Uri": "s3://demo/train/" },
            },
            "ContentType": "text/csv",
        }],
        "OutputDataConfig": { "S3OutputPath": output },
        "ResourceConfig": {
            "InstanceType": "ml.m5.large",
            "InstanceCount": 1,
            "VolumeSizeInGB": 10,
        },
        "StoppingCondition": { "MaxRuntimeInSeconds": 3600 },
    })
}

/// An `AlgorithmSpecification` naming the algorithm by its container image.
pub(crate) fn image_specification() -> Value {
    json!({ "TrainingImage": training_image(), "TrainingInputMode": "File" })
}

/// Register an algorithm through the implemented `CreateAlgorithm`, so a training job can
/// reference one that exists. Answers its ARN.
async fn create_algorithm(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .ok(
            "CreateAlgorithm",
            &json!({
                "AlgorithmName": name,
                "TrainingSpecification": {
                    "TrainingImage": training_image(),
                    "SupportedTrainingInstanceTypes": ["ml.m5.large"],
                    "TrainingChannels": [{
                        "Name": "train",
                        "SupportedContentTypes": ["text/csv"],
                        "SupportedInputModes": ["File"],
                    }],
                },
            }),
        )
        .await;
    str_field(&created, "AlgorithmArn")
}

/// RULE: an accepted training job is STORED, and `DescribeTrainingJob` replays it: the ARN
/// the create minted, the `InProgress` status SageMaker accepts a job in with the
/// secondary status of a job whose work has not begun, the model-artifact path derived
/// from the output location the request named, and the request's own members.
#[tokio::test]
async fn an_accepted_training_job_reads_back_through_describe() {
    // Rows differ along the name's shape, the output location's trailing separator (the
    // artifact path is one path however the caller wrote the prefix), and the optional
    // members the request carries.
    let rows: Vec<(&str, &str, &str, Value)> = vec![
        ("minimal", "t", "s3://demo/output/", json!({})),
        (
            "no trailing separator on the output path",
            "training-job-one",
            "s3://demo/nested/output",
            json!({}),
        ),
        (
            "hyperparameters, environment and tags",
            "a234567890123456789012345678901234567890123456789012345678901b",
            "s3://demo/output/",
            json!({
                "HyperParameters": { "eta": "0.2", "max_depth": "6" },
                "Environment": { "VERBOSE": "1" },
                "Tags": [{ "Key": "team", "Value": "science" }],
                "EnableNetworkIsolation": true,
                "EnableManagedSpotTraining": false,
                "RetryStrategy": { "MaximumRetryAttempts": 2 },
            }),
        ),
    ];

    for (case, name, output, extra) in rows {
        let emulator = Emulator::new();
        let mut body = training_body(name, image_specification(), output);
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            body[key] = value.clone();
        }

        let created = emulator.ok("CreateTrainingJob", &body).await;
        assert_eq!(
            str_field(&created, "TrainingJobArn"),
            arn("training-job", name),
            "case {case:?}: the minted ARN, got {created}"
        );

        let described = emulator
            .ok("DescribeTrainingJob", &json!({ "TrainingJobName": name }))
            .await;
        assert_eq!(
            (
                str_field(&described, "TrainingJobName"),
                str_field(&described, "TrainingJobArn"),
                str_field(&described, "TrainingJobStatus"),
                str_field(&described, "SecondaryStatus"),
            ),
            (
                name.to_string(),
                arn("training-job", name),
                "InProgress".to_string(),
                "Starting".to_string(),
            ),
            "case {case:?}: the job a read straight after the create reports, got {described}"
        );
        assert_eq!(
            described
                .get("ModelArtifacts")
                .map(|artifacts| str_field(artifacts, "S3ModelArtifacts")),
            Some(format!(
                "{}/{name}/output/model.tar.gz",
                output.trim_end_matches('/')
            )),
            "case {case:?}: the artifact path is derived from the output location, got \
             {described}"
        );
        // Nothing ran the image, so the members that would report a run's own
        // measurements carry no value at all.
        for absent in [
            "TrainingStartTime",
            "TrainingTimeInSeconds",
            "BillableTimeInSeconds",
            "FinalMetricDataList",
            "FailureReason",
        ] {
            assert!(
                described.get(absent).is_none(),
                "case {case:?}: {absent} must be absent while nothing ran, got {described}"
            );
        }
        // Every request member the describe models is replayed from the store. The
        // members the SERVICE supplies on top of the request (the channel and output
        // defaults, the switches, the profiling status, the status history) are pinned in
        // `service_resolved_members`, so this suite compares only what the request sent.
        for member in ["RoleArn", "ResourceConfig", "StoppingCondition"] {
            assert_eq!(
                described.get(member),
                body.get(member),
                "case {case:?}: {member} must be replayed as stored, got {described}"
            );
        }
        for member in ["AlgorithmSpecification", "OutputDataConfig"] {
            let sent = body
                .get(member)
                .and_then(Value::as_object)
                .map(|sent| sent.clone().into_iter().collect::<Vec<_>>())
                .unwrap_or_default();
            for (key, value) in sent {
                assert_eq!(
                    described.get(member).and_then(|reported| reported.get(&key)),
                    Some(&value),
                    "case {case:?}: {member}.{key} must be replayed as stored, got {described}"
                );
            }
        }
        // Every channel the request carried is replayed, in order, with the members it
        // stated unchanged.
        let sent_channels = body
            .get("InputDataConfig")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let reported_channels = described
            .get("InputDataConfig")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            sent_channels.len(),
            reported_channels.len(),
            "case {case:?}: every channel must be replayed, got {described}"
        );
        for (index, (sent, reported)) in sent_channels.iter().zip(&reported_channels).enumerate() {
            for key in ["ChannelName", "ContentType"] {
                assert_eq!(
                    reported.get(key),
                    sent.get(key),
                    "case {case:?}: InputDataConfig[{index}].{key} must be replayed as \
                     stored, got {reported}"
                );
            }
            assert_eq!(
                reported.pointer("/DataSource/S3DataSource/S3Uri"),
                sent.pointer("/DataSource/S3DataSource/S3Uri"),
                "case {case:?}: InputDataConfig[{index}]'s S3Uri must be replayed as \
                 stored, got {reported}"
            );
        }
        for member in extra.as_object().expect("a JSON object of extra members").keys() {
            if member == "Tags" {
                // `DescribeTrainingJob` models no `Tags` member; the tag operations own
                // that surface.
                continue;
            }
            assert_eq!(
                described.get(member),
                body.get(member),
                "case {case:?}: {member} must be replayed as stored, got {described}"
            );
        }

        // A name already in use is refused, and the stored job is untouched.
        let (status, refused) = emulator.call("CreateTrainingJob", &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ResourceInUse"),
            "case {case:?}: a second create under the same name, got {status} with {refused}"
        );
        let reread = emulator
            .ok("DescribeTrainingJob", &json!({ "TrainingJobName": name }))
            .await;
        assert_eq!(
            reread, described,
            "case {case:?}: a refused create must leave the job as it was, got {reread}"
        );
    }
}

/// RULE: the move out of `InProgress` is owed by the CLOCK, and the terminal status is the
/// one the job's own RUN reported — so a job whose training data could not be read lands in
/// the modelled failure status carrying the `ClientError:` reason that names the download leg
/// it failed on, the channel and its location, never a `Completed` status with artifacts,
/// metrics or a billable time for work that produced nothing.
///
/// This suite mounts SageMaker alone, so there is no Amazon S3 holding the channel's objects:
/// what the run reads is therefore unreadable, which is exactly the condition asserted here.
/// The COMPLETED path, with Amazon S3 and IAM mounted beside SageMaker, is
/// `crate::training_job_execution`.
#[tokio::test]
async fn the_clock_settles_a_training_job_into_its_terminal_status() {
    let emulator = Emulator::new();
    let name = "settling-job";
    emulator
        .ok(
            "CreateTrainingJob",
            &training_body(name, image_specification(), "s3://demo/output/"),
        )
        .await;

    tokio::time::sleep(PAST_ENDS_DUE).await;
    let settled = emulator
        .ok("DescribeTrainingJob", &json!({ "TrainingJobName": name }))
        .await;
    assert_eq!(
        (
            str_field(&settled, "TrainingJobStatus"),
            str_field(&settled, "SecondaryStatus"),
        ),
        ("Failed".to_string(), "Failed".to_string()),
        "case settled: the terminal status of a job whose data could not be read, got \
         {settled}"
    );
    let reason = settled
        .get("FailureReason")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let expected_prefix = "ClientError: Data download failed:";
    assert!(
        reason.starts_with(expected_prefix)
            && reason.contains("for channel train")
            && reason.contains("s3://demo/train/"),
        "case settled: the reason must start with {expected_prefix:?} and name the channel \
         and its location, got {reason:?}"
    );
    assert_eq!(
        settled
            .get("ModelArtifacts")
            .map(|artifacts| str_field(artifacts, "S3ModelArtifacts")),
        Some(format!("s3://demo/output/{name}/output/model.tar.gz")),
        "case settled: a failed run reports the location it would have written to, got \
         {settled}"
    );
    assert!(
        settled.get("FinalMetricDataList").is_none(),
        "case settled: a failed run reports no metric it did not measure, got {settled}"
    );

    // The transition is the clock's, so a later read answers the same thing.
    let reread = emulator
        .ok("DescribeTrainingJob", &json!({ "TrainingJobName": name }))
        .await;
    assert_eq!(
        reread, settled,
        "case settled: a settled job must read back identically, got {reread}"
    );
}

/// RULE: `AlgorithmSpecification` names the training algorithm EXACTLY ONCE — by a
/// registered `AlgorithmName` or by a `TrainingImage` — and the `AlgorithmName` reference
/// is resolved BEFORE the exclusion is weighed, so a request naming an algorithm the
/// account does not hold is refused for that whatever else the spec carries. Every refusal
/// is a 400 `ValidationException` and stores no job.
#[tokio::test]
async fn an_algorithm_specification_names_the_algorithm_exactly_once() {
    let emulator = Emulator::new();
    let registered = create_algorithm(&emulator, "registered-algo").await;

    // Rows differ along which members the spec carries, whether the named algorithm
    // exists, and the form of the name (bare name or full ARN, both `ArnOrName`).
    let rows: Vec<(&str, Value, Option<String>)> = vec![
        (
            "an unknown algorithm name beside a training image",
            json!({
                "AlgorithmName": "absent-algo",
                "TrainingImage": training_image(),
                "TrainingInputMode": "File",
            }),
            Some(algorithm_missing("absent-algo")),
        ),
        (
            "an unknown algorithm name alone",
            json!({ "AlgorithmName": "other-absent-algo", "TrainingInputMode": "File" }),
            Some(algorithm_missing("other-absent-algo")),
        ),
        (
            "an unknown algorithm named by its ARN",
            json!({
                "AlgorithmName": arn("algorithm", "arn-absent-algo"),
                "TrainingInputMode": "File",
            }),
            Some(algorithm_missing("arn-absent-algo")),
        ),
        (
            "a registered algorithm beside a training image",
            json!({
                "AlgorithmName": "registered-algo",
                "TrainingImage": training_image(),
                "TrainingInputMode": "File",
            }),
            Some(
                "AlgorithmSpecification may name only one of AlgorithmName and TrainingImage."
                    .to_string(),
            ),
        ),
        (
            "neither an algorithm name nor a training image",
            json!({ "TrainingInputMode": "File" }),
            Some(
                "AlgorithmSpecification must name either AlgorithmName or TrainingImage."
                    .to_string(),
            ),
        ),
        (
            "a registered algorithm alone",
            json!({ "AlgorithmName": "registered-algo", "TrainingInputMode": "File" }),
            None,
        ),
        (
            "a registered algorithm named by its ARN",
            json!({ "AlgorithmName": registered, "TrainingInputMode": "File" }),
            None,
        ),
        ("a training image alone", image_specification(), None),
    ];

    for (index, (case, specification, expected)) in rows.into_iter().enumerate() {
        let name = format!("spec-case-{index}");
        let body = training_body(&name, specification, "s3://demo/output/");
        let (status, answer) = emulator.call("CreateTrainingJob", &body).await;

        let Some(expected) = expected else {
            assert_eq!(
                status, 200,
                "case {case:?}: a spec naming the algorithm once is accepted, got {status} \
                 with {answer}"
            );
            let described = emulator
                .ok("DescribeTrainingJob", &json!({ "TrainingJobName": name }))
                .await;
            assert_eq!(
                str_field(&described, "TrainingJobStatus"),
                "InProgress",
                "case {case:?}: the accepted job is in progress, got {described}"
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
            expected,
            "case {case:?}: the refusal's message, got {answer}"
        );

        // No job was created: its reader says so, with the fixed sentence the
        // training-job readers answer for a name the account holds no job under.
        let (status, described) = emulator
            .call("DescribeTrainingJob", &json!({ "TrainingJobName": name }))
            .await;
        assert_eq!(
            (status, error_code(&described).as_str()),
            (400, "ValidationException"),
            "case {case:?}: a refused create stores no job, got {status} with {described}"
        );
        assert_eq!(
            message(&described),
            service_com_amazonaws_sagemaker_sagemaker::state::REQUESTED_RESOURCE_NOT_FOUND,
            "case {case:?}: the reader's refusal for an absent job, got {described}"
        );
    }
}

/// RULE: an algorithm the account does not hold is stated the same way by every operation
/// that resolves an algorithm reference — the entity label, the identifier resolved to its
/// ARN, then `does not exist.` — whichever `ArnOrName` form the request carried and
/// whether the name was never used or has just been deleted.
#[tokio::test]
async fn an_absent_algorithm_is_stated_the_same_way_by_every_reference() {
    let emulator = Emulator::new();
    // One algorithm is created and deleted, so the rows cover a name that was never used
    // AND one the account held a moment ago.
    create_algorithm(&emulator, "deleted-algo").await;
    // The delete is only accepted once the algorithm's registration has settled, which
    // `crate::algorithm_lifecycle` pins; here it just has to get the name released.
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    emulator
        .ok("DeleteAlgorithm", &json!({ "AlgorithmName": "deleted-algo" }))
        .await;

    // Rows differ along the operation that resolves the reference, the identifier form,
    // and whether the name was ever in use.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "describe a name never used",
            "DescribeAlgorithm",
            json!({ "AlgorithmName": "never-used-algo" }),
            "never-used-algo",
        ),
        (
            "describe a name just deleted",
            "DescribeAlgorithm",
            json!({ "AlgorithmName": "deleted-algo" }),
            "deleted-algo",
        ),
        (
            "describe by ARN",
            "DescribeAlgorithm",
            json!({ "AlgorithmName": arn("algorithm", "deleted-algo") }),
            "deleted-algo",
        ),
        (
            "delete a name just deleted",
            "DeleteAlgorithm",
            json!({ "AlgorithmName": "deleted-algo" }),
            "deleted-algo",
        ),
        (
            "create a training job referencing it",
            "CreateTrainingJob",
            training_body(
                "referencing-job",
                json!({ "AlgorithmName": "deleted-algo", "TrainingInputMode": "File" }),
                "s3://demo/output/",
            ),
            "deleted-algo",
        ),
    ];

    for (case, operation, body, resolved) in rows {
        let (status, refused) = emulator.call(operation, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {refused}"
        );
        assert_eq!(
            message(&refused),
            algorithm_missing(resolved),
            "case {case:?}: the refusal's message, got {refused}"
        );
    }
}
