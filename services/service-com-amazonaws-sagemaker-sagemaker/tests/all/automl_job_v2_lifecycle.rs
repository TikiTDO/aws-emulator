// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's AutoML job V2 lifecycle:
//! `CreateAutoMLJobV2` -> `DescribeAutoMLJobV2`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness (`Emulator`,
//! `error_code`, `str_field`) is the one `inference_deployment_lifecycle` already
//! defines for this target.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the
//! create/describe round trip for EVERY `AutoMLProblemTypeConfig` member and every
//! optional request member, the ARN form for every region and name shape, the
//! case-insensitive uniqueness of `AutoMLJobName`, each documented cross-member
//! refusal, and the status transition the clock owes a job nothing trained.

use std::time::Duration;

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, str_field, Emulator, ACCOUNT};

/// Long enough to pass `state::AUTO_ML_JOB_TERMINAL_AFTER` (5 x 700 ms).
const PAST_TERMINAL_DUE: Duration = Duration::from_millis(6_000);

/// A `RoleArn` satisfying the modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/service-role/AmazonSageMaker-ExecutionRole";

/// `DescribeAutoMLJobV2` for `name`, requiring a 200.
async fn describe(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok("DescribeAutoMLJobV2", &json!({ "AutoMLJobName": name }))
        .await
}

/// One input channel: the `S3Prefix` source every problem type accepts.
fn channel(uri: &str) -> Value {
    json!({
        "ChannelType": "training",
        "ContentType": "text/csv;header=present",
        "CompressionType": "None",
        "DataSource": { "S3DataSource": { "S3DataType": "S3Prefix", "S3Uri": uri } },
    })
}

fn output_config() -> Value {
    json!({ "S3OutputPath": "s3://autopilot-output/jobs/" })
}

/// A minimal valid `CreateAutoMLJobV2` body for `name` carrying `problem_type_config`.
fn create_body(name: &str, problem_type_config: &Value) -> Value {
    json!({
        "AutoMLJobName": name,
        "AutoMLJobInputDataConfig": [channel("s3://autopilot-input/train/")],
        "OutputDataConfig": output_config(),
        "AutoMLProblemTypeConfig": problem_type_config,
        "RoleArn": ROLE_ARN,
    })
}

/// Every member of the `AutoMLProblemTypeConfig` union, with the
/// `AutoMLProblemTypeConfigName` the model binds to it.
///
/// Catalog source: `smithy_query(query="shape", shape="AutoMLProblemTypeConfig")` for the
/// members and `shape="AutoMLProblemTypeConfigName"` for the names (ImageClassification |
/// TextClassification | TimeSeriesForecasting | Tabular | TextGeneration). Each row
/// carries exactly that member's REQUIRED sub-members, so nothing is refused at the
/// boundary.
fn problem_type_configs() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "tabular",
            "Tabular",
            json!({ "TabularJobConfig": { "TargetAttributeName": "y" } }),
        ),
        (
            "image-classification",
            "ImageClassification",
            json!({ "ImageClassificationJobConfig": {} }),
        ),
        (
            "text-classification",
            "TextClassification",
            json!({
                "TextClassificationJobConfig": {
                    "ContentColumn": "review",
                    "TargetLabelColumn": "sentiment",
                }
            }),
        ),
        (
            "time-series-forecasting",
            "TimeSeriesForecasting",
            json!({
                "TimeSeriesForecastingJobConfig": {
                    "ForecastFrequency": "1D",
                    "ForecastHorizon": 7,
                    "TimeSeriesConfig": {
                        "TargetAttributeName": "demand",
                        "TimestampAttributeName": "ts",
                        "ItemIdentifierAttributeName": "item",
                    },
                }
            }),
        ),
        (
            "text-generation",
            "TextGeneration",
            json!({ "TextGenerationJobConfig": {} }),
        ),
    ]
}

/// RULE: `DescribeAutoMLJobV2` replays exactly what `CreateAutoMLJobV2` persisted, and
/// reports the `AutoMLProblemTypeConfigName` that names the stored
/// `AutoMLProblemTypeConfig` member — for every member of that union.
#[tokio::test]
async fn describe_replays_the_stored_job_for_every_problem_type_config() {
    for (case, config_name, config) in problem_type_configs() {
        let emulator = Emulator::new();
        let name = format!("job-{case}");
        // `AutoMLJobName` is len 1..32, so keep the row inside the modelled bound.
        let name = &name[..name.len().min(32)];

        let created = emulator
            .ok("CreateAutoMLJobV2", &create_body(name, &config))
            .await;
        assert_eq!(
            str_field(&created, "AutoMLJobArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:automl-job/{name}"),
            "case {case}: AutoMLJobArn, got {created:?}"
        );

        let described = describe(&emulator, name).await;
        assert_eq!(
            str_field(&described, "AutoMLJobName"),
            name,
            "case {case}: AutoMLJobName, got {described:?}"
        );
        assert_eq!(
            described.get("AutoMLJobArn"),
            created.get("AutoMLJobArn"),
            "case {case}: the described ARN must be the created one, got {described:?}"
        );
        assert_eq!(
            described.get("AutoMLProblemTypeConfig"),
            Some(&config),
            "case {case}: AutoMLProblemTypeConfig must replay what was stored, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "AutoMLProblemTypeConfigName"),
            config_name,
            "case {case}: AutoMLProblemTypeConfigName, got {described:?}"
        );
        assert_eq!(
            described.get("AutoMLJobInputDataConfig"),
            Some(&json!([channel("s3://autopilot-input/train/")])),
            "case {case}: AutoMLJobInputDataConfig, got {described:?}"
        );
        assert_eq!(
            described.get("OutputDataConfig"),
            Some(&output_config()),
            "case {case}: OutputDataConfig, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "RoleArn"),
            ROLE_ARN,
            "case {case}: RoleArn, got {described:?}"
        );
        // A job accepted this instant has not left its initial status, and nothing has
        // been trained, so no candidate, artifact or end time is reported.
        assert_eq!(
            str_field(&described, "AutoMLJobStatus"),
            "InProgress",
            "case {case}: AutoMLJobStatus, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "AutoMLJobSecondaryStatus"),
            "Starting",
            "case {case}: AutoMLJobSecondaryStatus, got {described:?}"
        );
        for absent in [
            "EndTime",
            "FailureReason",
            "BestCandidate",
            "AutoMLJobArtifacts",
            "ResolvedAttributes",
            "ModelDeployResult",
            "PartialFailureReasons",
        ] {
            assert!(
                described.get(absent).is_none(),
                "case {case}: {absent} must be absent on a job nothing has run, \
                 got {described:?}"
            );
        }
        let created_at = described
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: CreationTime in {described:?}"));
        let modified_at = described
            .get("LastModifiedTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: LastModifiedTime in {described:?}"));
        assert!(
            (modified_at - created_at).abs() < f64::EPSILON,
            "case {case}: LastModifiedTime {modified_at} must equal CreationTime \
             {created_at} before anything has changed"
        );
    }
}

/// RULE: the minted ARN is
/// `arn:aws:sagemaker:<signed region>:<account>:automl-job/<AutoMLJobName>`, whatever
/// region the caller signed for and whatever shape the name has.
#[tokio::test]
async fn the_minted_arn_names_the_signed_region_and_the_stored_name() {
    // Rows differ along the axes the rule generalises over: region, and the name's
    // shape — one character (the `len 1..32` lower bound), digits, hyphens, and the
    // 32-character upper bound (`smithy_query` operation=CreateAutoMLJobV2:
    // `AutoMLJobName` len 1..32, pattern `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,31}$`).
    let names = [
        "j",
        "job9",
        "autopilot-job-with-hyphens",
        "a2345678901234567890123456789012",
    ];
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        for name in names {
            let emulator = Emulator::new();
            let case = format!("region={region} name={name}");
            let (status, created) = emulator
                .call_in(
                    region,
                    "CreateAutoMLJobV2",
                    &create_body(name, &json!({ "TabularJobConfig": { "TargetAttributeName": "y" } })),
                )
                .await;
            assert_eq!(status, 200, "case {case}: CreateAutoMLJobV2, got {created:?}");
            assert_eq!(
                str_field(&created, "AutoMLJobArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:automl-job/{name}"),
                "case {case}: AutoMLJobArn, got {created:?}"
            );
        }
    }

    // One past the modelled upper bound, and a name the modelled pattern rejects: both
    // refused at the boundary, before any state is written.
    let emulator = Emulator::new();
    for (case, name) in [
        ("name-past-32", "a23456789012345678901234567890123"),
        ("name-with-an-illegal-character", "not a job name"),
        ("empty-name", ""),
    ] {
        let (status, body) = emulator
            .call(
                "CreateAutoMLJobV2",
                &create_body(name, &json!({ "TabularJobConfig": { "TargetAttributeName": "y" } })),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: CreateAutoMLJobV2 must be refused, got {status} {body}"
        );
    }
}

/// RULE: every optional request member a job was created with is replayed by the
/// describe, and one it was not created with is omitted.
#[tokio::test]
async fn every_optional_member_round_trips_through_the_describe() {
    let emulator = Emulator::new();

    // Rows: each optional member of `CreateAutoMLJobV2Request` (smithy_query
    // operation=CreateAutoMLJobV2), one row per member and one row carrying all of them,
    // against a row carrying none.
    let objective = json!({ "MetricName": "F1" });
    let security = json!({
        "VolumeKmsKeyId": "alias/autopilot",
        "EnableInterContainerTrafficEncryption": true,
        "VpcConfig": { "SecurityGroupIds": ["sg-01"], "Subnets": ["subnet-01"] },
    });
    let deploy = json!({ "AutoGenerateEndpointName": true });
    let split = json!({ "ValidationFraction": 0.25 });
    let compute = json!({
        "EmrServerlessComputeConfig": {
            "ExecutionRoleARN": "arn:aws:iam::123456789012:role/emr-serverless",
        }
    });
    let tags = json!([{ "Key": "team", "Value": "ml" }, { "Key": "env", "Value": "test" }]);

    let cases: Vec<(&str, Vec<(&str, Value)>)> = vec![
        ("none", vec![]),
        ("objective-and-problem-type", vec![("AutoMLJobObjective", objective.clone())]),
        ("security-config", vec![("SecurityConfig", security.clone())]),
        ("model-deploy-config", vec![("ModelDeployConfig", deploy.clone())]),
        ("data-split-config", vec![("DataSplitConfig", split.clone())]),
        ("compute-config", vec![("AutoMLComputeConfig", compute.clone())]),
        ("tags", vec![("Tags", tags.clone())]),
        (
            "all-of-them",
            vec![
                ("AutoMLJobObjective", objective.clone()),
                ("SecurityConfig", security.clone()),
                ("ModelDeployConfig", deploy.clone()),
                ("DataSplitConfig", split.clone()),
                ("AutoMLComputeConfig", compute.clone()),
                ("Tags", tags.clone()),
            ],
        ),
    ];

    // The members the describe reports back, with the request member each comes from.
    let echoed = [
        "AutoMLJobObjective",
        "SecurityConfig",
        "ModelDeployConfig",
        "DataSplitConfig",
        "AutoMLComputeConfig",
    ];

    for (index, (case, members)) in cases.iter().enumerate() {
        let name = format!("opt{index}");
        // The tabular objective rule pairs `AutoMLJobObjective` with
        // `TabularJobConfig.ProblemType`, so a row carrying the objective carries both.
        let tabular = if members.iter().any(|(member, _)| *member == "AutoMLJobObjective") {
            json!({
                "TabularJobConfig": {
                    "TargetAttributeName": "y",
                    "ProblemType": "BinaryClassification",
                }
            })
        } else {
            json!({ "TabularJobConfig": { "TargetAttributeName": "y" } })
        };
        let mut body = create_body(&name, &tabular);
        for (member, value) in members {
            body[*member] = value.clone();
        }

        let (status, created) = emulator.call("CreateAutoMLJobV2", &body).await;
        assert_eq!(
            status, 200,
            "case {case}: CreateAutoMLJobV2, got {status} {created}"
        );

        let described = describe(&emulator, &name).await;
        for member in echoed {
            let sent = body.get(member);
            assert_eq!(
                described.get(member),
                sent,
                "case {case}: {member} must replay what was stored, got {described:?}"
            );
        }
        // Tags are not part of the describe's output shape, so they are never reported
        // there; what matters is that carrying them changes nothing else.
        assert_eq!(
            str_field(&described, "AutoMLJobName"),
            name,
            "case {case}: AutoMLJobName, got {described:?}"
        );
    }
}

/// RULE: `AutoMLJobName` is unique per account and CASE INSENSITIVE, so a second create
/// whose name differs only in case is refused with the modelled `ResourceInUse` and
/// leaves the first job exactly as it was — and the describe finds that one job whatever
/// case it asks for.
#[tokio::test]
async fn a_name_differing_only_in_case_names_the_same_job() {
    let emulator = Emulator::new();
    let tabular = json!({ "TabularJobConfig": { "TargetAttributeName": "y" } });
    let stored = "AutoPilotJob";
    emulator
        .ok("CreateAutoMLJobV2", &create_body(stored, &tabular))
        .await;
    let first = describe(&emulator, stored).await;

    // Rows: the same name in each case form the pattern `^[a-zA-Z0-9](-*[a-zA-Z0-9])*$`
    // allows, including the one originally stored.
    for case in ["AutoPilotJob", "autopilotjob", "AUTOPILOTJOB", "aUtOpIlOtJoB"] {
        let (status, body) = emulator
            .call("CreateAutoMLJobV2", &create_body(case, &tabular))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a duplicate name must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceInUse",
            "case {case}: error code, got {body:?}"
        );

        let described = describe(&emulator, case).await;
        assert_eq!(
            str_field(&described, "AutoMLJobName"),
            stored,
            "case {case}: the describe must find the one stored job and report the name \
             as first spelled, got {described:?}"
        );
        assert_eq!(
            described.get("CreationTime"),
            first.get("CreationTime"),
            "case {case}: the refused create must not have replaced the stored job, \
             got {described:?}"
        );
    }
}

/// RULE: describing a job this account does not hold is refused with the modelled
/// `ResourceNotFound`.
#[tokio::test]
async fn describing_an_absent_job_is_refused_with_resource_not_found() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateAutoMLJobV2",
            &create_body(
                "present",
                &json!({ "TabularJobConfig": { "TargetAttributeName": "y" } }),
            ),
        )
        .await;

    // Rows: a name never created, a name that is a prefix of the stored one, and one it
    // is a prefix of — none of which is the stored job.
    for case in ["absent", "pres", "present2"] {
        let (status, body) = emulator
            .call("DescribeAutoMLJobV2", &json!({ "AutoMLJobName": case }))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: DescribeAutoMLJobV2 must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFound",
            "case {case}: error code, got {body:?}"
        );
    }
}

/// RULE: a job is accepted in `InProgress`/`Starting`, and the terminal transition the
/// clock owes it becomes visible once its span has elapsed — without any call having to
/// make it happen, and stably thereafter. The terminal status is the one the job's own RUN
/// reported: these jobs name an Amazon S3 location nothing holds an object under, so the
/// run's data-download leg fails and the job ends `Failed` with the condition that stopped
/// it, never `Completed` (the completed path, over data Amazon S3 really holds, is
/// `automl_job_v2_execution`).
#[tokio::test]
async fn a_job_leaves_in_progress_once_the_clock_owes_it_and_reports_why_it_failed() {
    let emulator = Emulator::new();
    // Rows: every problem type — none of them has data to read, so all of them end the
    // same way.
    for (case, _, config) in problem_type_configs() {
        let name = format!("t-{case}");
        let name = &name[..name.len().min(32)];
        emulator
            .ok("CreateAutoMLJobV2", &create_body(name, &config))
            .await;

        let running = describe(&emulator, name).await;
        assert_eq!(
            str_field(&running, "AutoMLJobStatus"),
            "InProgress",
            "case {case} immediately-after-create: AutoMLJobStatus, got {running:?}"
        );

        tokio::time::sleep(PAST_TERMINAL_DUE).await;

        let settled = describe(&emulator, name).await;
        assert_eq!(
            str_field(&settled, "AutoMLJobStatus"),
            "Failed",
            "case {case} after-the-due-span: AutoMLJobStatus, got {settled:?}"
        );
        assert_eq!(
            str_field(&settled, "AutoMLJobSecondaryStatus"),
            "Failed",
            "case {case} after-the-due-span: AutoMLJobSecondaryStatus, got {settled:?}"
        );
        let reason = str_field(&settled, "FailureReason");
        assert!(
            reason.starts_with("ClientError:") && reason.contains("autopilot-input"),
            "case {case} after-the-due-span: FailureReason must name the leg of the run \
             that failed and the location it failed on, got {reason:?}"
        );
        assert!(
            settled.get("BestCandidate").is_none()
                && settled.get("AutoMLJobArtifacts").is_none(),
            "case {case} after-the-due-span: a failed job reports no candidate and no \
             artifacts, got {settled:?}"
        );

        let created_at = settled
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: CreationTime in {settled:?}"));
        let ended_at = settled
            .get("EndTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: EndTime in {settled:?}"));
        let modified_at = settled
            .get("LastModifiedTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: LastModifiedTime in {settled:?}"));
        assert!(
            ended_at > created_at,
            "case {case}: EndTime {ended_at} must follow CreationTime {created_at}"
        );
        assert!(
            (modified_at - ended_at).abs() < f64::EPSILON,
            "case {case}: LastModifiedTime {modified_at} must be the instant the \
             transition fell due, {ended_at}"
        );
        assert_eq!(
            settled.get("CreationTime"),
            running.get("CreationTime"),
            "case {case}: CreationTime must not move, got {settled:?}"
        );

        let again = describe(&emulator, name).await;
        assert_eq!(
            again, settled,
            "case {case} repeated-read: a settled job must read back identically"
        );
    }
}

/// One row of the documented-combination table: the case name, the HTTP status the
/// request must get, the `AutoMLProblemTypeConfig` it carries, and the optional request
/// members it sets on top of the minimal body.
type ComboCase = (&'static str, u16, Value, Vec<(&'static str, Value)>);

/// RULE: a combination of members the operation's own docs forbid is refused with HTTP
/// 400 `ValidationException` and persists nothing; every combination they allow is
/// accepted.
#[tokio::test]
async fn documented_member_combinations_are_enforced() {
    let tabular = json!({ "TabularJobConfig": { "TargetAttributeName": "y" } });
    let tabular_with_problem_type = json!({
        "TabularJobConfig": { "TargetAttributeName": "y", "ProblemType": "Regression" }
    });
    let time_series = json!({
        "TimeSeriesForecastingJobConfig": {
            "ForecastFrequency": "1D",
            "ForecastHorizon": 7,
            "TimeSeriesConfig": {
                "TargetAttributeName": "demand",
                "TimestampAttributeName": "ts",
                "ItemIdentifierAttributeName": "item",
            },
        }
    });
    let objective = json!({ "MetricName": "MSE" });
    let split = json!({ "ValidationFraction": 0.2 });

    // Rows: each side of every documented rule, and the allowed combinations beside the
    // forbidden ones.
    //   * ModelDeployConfig: "Specify the EndpointName if and only if you set
    //     AutoGenerateEndpointName to False; otherwise a 400 error is thrown."
    //   * DataSplitConfig: "must not be set for the time-series forecasting problem type".
    //   * AutoMLJobObjective: for tabular, "either provide both the AutoMLJobObjective
    //     and ... TabularJobConfig.ProblemType, or none at all".
    //   * Tags: "Tag keys must be unique per resource."
    let cases: Vec<ComboCase> = vec![
        (
            "auto-generate-endpoint-name-with-an-endpoint-name",
            400,
            tabular.clone(),
            vec![(
                "ModelDeployConfig",
                json!({ "AutoGenerateEndpointName": true, "EndpointName": "ep" }),
            )],
        ),
        (
            "auto-generate-endpoint-name-alone",
            200,
            tabular.clone(),
            vec![(
                "ModelDeployConfig",
                json!({ "AutoGenerateEndpointName": true }),
            )],
        ),
        (
            "endpoint-name-with-auto-generate-false",
            200,
            tabular.clone(),
            vec![(
                "ModelDeployConfig",
                json!({ "AutoGenerateEndpointName": false, "EndpointName": "ep" }),
            )],
        ),
        (
            "auto-generate-false-and-no-endpoint-name",
            400,
            tabular.clone(),
            vec![(
                "ModelDeployConfig",
                json!({ "AutoGenerateEndpointName": false }),
            )],
        ),
        (
            "empty-model-deploy-config",
            400,
            tabular.clone(),
            vec![("ModelDeployConfig", json!({}))],
        ),
        (
            "data-split-config-on-time-series",
            400,
            time_series.clone(),
            vec![("DataSplitConfig", split.clone())],
        ),
        (
            "data-split-config-on-tabular",
            200,
            tabular.clone(),
            vec![("DataSplitConfig", split.clone())],
        ),
        (
            "tabular-objective-without-a-problem-type",
            400,
            tabular.clone(),
            vec![("AutoMLJobObjective", objective.clone())],
        ),
        (
            "tabular-problem-type-without-an-objective",
            400,
            tabular_with_problem_type.clone(),
            vec![],
        ),
        (
            "tabular-objective-and-problem-type-together",
            200,
            tabular_with_problem_type.clone(),
            vec![("AutoMLJobObjective", objective.clone())],
        ),
        ("tabular-with-neither", 200, tabular.clone(), vec![]),
        (
            // The pairing rule is stated for tabular problem types only, and no other
            // problem type has a `ProblemType` member to pair with.
            "text-generation-objective-alone",
            200,
            json!({ "TextGenerationJobConfig": {} }),
            vec![("AutoMLJobObjective", objective.clone())],
        ),
        (
            "time-series-objective-alone",
            200,
            time_series.clone(),
            vec![("AutoMLJobObjective", json!({ "MetricName": "RMSE" }))],
        ),
        (
            // `Tags`' doc: "Tag keys must be unique per resource."
            "tags-taking-one-key-twice",
            400,
            tabular.clone(),
            vec![(
                "Tags",
                json!([{ "Key": "team", "Value": "ml" }, { "Key": "team", "Value": "other" }]),
            )],
        ),
        (
            "tags-with-distinct-keys",
            200,
            tabular.clone(),
            vec![(
                "Tags",
                json!([{ "Key": "team", "Value": "ml" }, { "Key": "env", "Value": "test" }]),
            )],
        ),
    ];

    for (index, (case, expected, config, members)) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("combo{index}");
        let mut body = create_body(&name, config);
        for (member, value) in members {
            body[*member] = value.clone();
        }

        let (status, parsed) = emulator.call("CreateAutoMLJobV2", &body).await;
        assert_eq!(
            status, *expected,
            "case {case}: expected HTTP {expected}, got {status} with {parsed}"
        );
        if *expected == 400 {
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case {case}: error code, got {parsed:?}"
            );
            let (describe_status, described) = emulator
                .call("DescribeAutoMLJobV2", &json!({ "AutoMLJobName": name }))
                .await;
            assert_eq!(
                describe_status, 400,
                "case {case}: a refused create must persist nothing, got \
                 {describe_status} {described}"
            );
        }
    }
}

/// RULE: the modelled bounds on `AutoMLJobInputDataConfig` (len 1..2) are enforced at the
/// boundary, on each published bound and one past it.
#[tokio::test]
async fn the_input_data_config_bounds_are_enforced() {
    let emulator = Emulator::new();
    let tabular = json!({ "TabularJobConfig": { "TargetAttributeName": "y" } });
    let channels = [
        channel("s3://autopilot-input/train/"),
        channel("s3://autopilot-input/validation/"),
        channel("s3://autopilot-input/extra/"),
    ];

    // Rows: 0 (below the lower bound), 1 and 2 (the bounds themselves), 3 (one past the
    // upper bound).
    for (count, expected) in [(0usize, 400u16), (1, 200), (2, 200), (3, 400)] {
        let name = format!("chan{count}");
        let mut body = create_body(&name, &tabular);
        body["AutoMLJobInputDataConfig"] = Value::Array(channels[..count].to_vec());

        let (status, parsed) = emulator.call("CreateAutoMLJobV2", &body).await;
        assert_eq!(
            status, expected,
            "case channels={count}: expected HTTP {expected}, got {status} with {parsed}"
        );
        if expected == 200 {
            let described = describe(&emulator, &name).await;
            assert_eq!(
                described.get("AutoMLJobInputDataConfig"),
                Some(&Value::Array(channels[..count].to_vec())),
                "case channels={count}: the stored channels must replay in order, \
                 got {described:?}"
            );
        }
    }
}
