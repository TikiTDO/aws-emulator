// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's AI benchmarking jobs:
//! `CreateAIBenchmarkJob` -> `DescribeAIBenchmarkJob`, beside the
//! `CreateAIWorkloadConfig` each job references.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the ARN
//! form for every region and name shape; the create/describe round trip for every
//! combination of the optional and union-valued members; the single clock-owed move out
//! of `InProgress`; the refusal of a name already in use; the refusal of a job whose
//! workload configuration does not exist; and the refusal of a read of a name that was
//! never created.

use std::time::Duration;

use serde_json::{json, Value};

// The crate's wire-test harness, exposed `pub(crate)` by the suite that introduced it so
// every sibling module in this one test binary shares one emulator, one request builder
// and one set of body accessors.
use crate::inference_deployment_lifecycle::{
    error_code, one_variant_config, primary_container, str_field, Emulator, ACCOUNT,
};

/// Long enough to pass `state::AI_BENCHMARK_JOB_ENDS_AFTER` (400 ms).
const PAST_JOB_END_DUE: Duration = Duration::from_millis(700);

/// A `RoleArn` matching the modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`
/// (smithy_query operation=CreateAIBenchmarkJob).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/SageMakerBenchmarkRole";

/// The workload configuration every job below benchmarks against.
const WORKLOAD_CONFIG: &str = "bench-workload-config";

/// The inference endpoint every job below targets. `CreateAIBenchmarkJob` resolves its
/// `BenchmarkTarget` against the endpoint store, so the endpoint is seeded beside the
/// workload configuration.
const TARGET_ENDPOINT: &str = "llama-endpoint";

/// `Tags` with `count` distinct key/value pairs.
fn tags(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({ "Key": format!("k{index}"), "Value": format!("v{index}") }))
            .collect(),
    )
}

/// One row of the round-trip table: a case name and the members a
/// `CreateAIBenchmarkJob` request carries beyond `AIBenchmarkJobName`.
struct Row {
    case: &'static str,
    members: Value,
}

/// The round-trip table. Rows differ along the axes the round-trip rule generalises
/// over: which optional members are present (`NetworkConfig`, `Tags`,
/// `OutputConfig$MlflowConfig`), how the `BenchmarkTarget` union's `Endpoint` member is
/// filled (bare identifier, target container hostname, one and several inference
/// components), whether the workload configuration is named by name or by ARN, the form
/// of `S3OutputLocation` (`s3://` and `https://`, both admitted by `S3Uri`'s modelled
/// pattern `^(https|s3)://([^/]+)/?(.*)$`), and how many tags there are (0, 1 and the
/// modelled `TagList` maximum of 50).
fn round_trip_rows() -> Vec<Row> {
    vec![
        Row {
            case: "required members only",
            members: json!({
                "AIWorkloadConfigIdentifier": WORKLOAD_CONFIG,
                "RoleArn": ROLE_ARN,
                "BenchmarkTarget": { "Endpoint": { "Identifier": "llama-endpoint" } },
                "OutputConfig": { "S3OutputLocation": "s3://bucket/benchmarks/" },
            }),
        },
        Row {
            case: "workload configuration named by ARN",
            members: json!({
                "AIWorkloadConfigIdentifier": format!(
                    "arn:aws:sagemaker:us-east-1:{ACCOUNT}:ai-workload-config/{WORKLOAD_CONFIG}"
                ),
                "RoleArn": ROLE_ARN,
                "BenchmarkTarget": { "Endpoint": { "Identifier": "llama-endpoint" } },
                "OutputConfig": { "S3OutputLocation": "s3://bucket/benchmarks/" },
            }),
        },
        Row {
            case: "endpoint target with a container hostname",
            members: json!({
                "AIWorkloadConfigIdentifier": WORKLOAD_CONFIG,
                "RoleArn": ROLE_ARN,
                "BenchmarkTarget": {
                    "Endpoint": {
                        "Identifier": "llama-endpoint",
                        "TargetContainerHostname": "container-1",
                    },
                },
                "OutputConfig": { "S3OutputLocation": "https://bucket.s3.amazonaws.com/out/" },
            }),
        },
        Row {
            case: "endpoint target with three inference components",
            members: json!({
                "AIWorkloadConfigIdentifier": WORKLOAD_CONFIG,
                "RoleArn": ROLE_ARN,
                "BenchmarkTarget": {
                    "Endpoint": {
                        "Identifier": format!(
                            "arn:aws:sagemaker:us-east-1:{ACCOUNT}:endpoint/llama-endpoint"
                        ),
                        "InferenceComponents": [
                            { "Identifier": "component-a" },
                            { "Identifier": "component-b" },
                            { "Identifier": "component-c" },
                        ],
                    },
                },
                "OutputConfig": { "S3OutputLocation": "s3://bucket/benchmarks/run-1" },
            }),
        },
        Row {
            case: "output configuration with an MLflow tracking destination",
            members: json!({
                "AIWorkloadConfigIdentifier": WORKLOAD_CONFIG,
                "RoleArn": ROLE_ARN,
                "BenchmarkTarget": { "Endpoint": { "Identifier": "llama-endpoint" } },
                "OutputConfig": {
                    "S3OutputLocation": "s3://bucket/benchmarks/",
                    "MlflowConfig": {
                        // `AIMlflowResourceArn`'s modelled pattern admits exactly the
                        // `mlflow-app` and `mlflow-tracking-server` kinds.
                        "MlflowResourceArn": format!(
                            "arn:aws:sagemaker:us-east-1:{ACCOUNT}:mlflow-tracking-server/mlflow-1"
                        ),
                        "MlflowExperimentName": "bench.experiment-1",
                        "MlflowRunName": "run_1",
                    },
                },
            }),
        },
        Row {
            case: "network configuration with a VPC",
            members: json!({
                "AIWorkloadConfigIdentifier": WORKLOAD_CONFIG,
                "RoleArn": ROLE_ARN,
                "BenchmarkTarget": { "Endpoint": { "Identifier": "llama-endpoint" } },
                "OutputConfig": { "S3OutputLocation": "s3://bucket/benchmarks/" },
                "NetworkConfig": {
                    "VpcConfig": {
                        "SecurityGroupIds": ["sg-0123456789abcdef0"],
                        "Subnets": ["subnet-0123456789abcdef0", "subnet-00fedcba9876543210"],
                    },
                },
            }),
        },
        Row {
            case: "empty tag list",
            members: json!({
                "AIWorkloadConfigIdentifier": WORKLOAD_CONFIG,
                "RoleArn": ROLE_ARN,
                "BenchmarkTarget": { "Endpoint": { "Identifier": "llama-endpoint" } },
                "OutputConfig": { "S3OutputLocation": "s3://bucket/benchmarks/" },
                "Tags": tags(0),
            }),
        },
        Row {
            case: "fifty tags (TagList maximum)",
            members: json!({
                "AIWorkloadConfigIdentifier": WORKLOAD_CONFIG,
                "RoleArn": ROLE_ARN,
                "BenchmarkTarget": { "Endpoint": { "Identifier": "llama-endpoint" } },
                "OutputConfig": { "S3OutputLocation": "s3://bucket/benchmarks/" },
                "Tags": tags(50),
            }),
        },
        Row {
            case: "every member at once",
            members: json!({
                "AIWorkloadConfigIdentifier": WORKLOAD_CONFIG,
                "RoleArn": "arn:aws:iam::000000000000:role/service-role/Bench+Role.1",
                "BenchmarkTarget": {
                    "Endpoint": {
                        "Identifier": "llama-endpoint",
                        "TargetContainerHostname": "container-1",
                        "InferenceComponents": [{ "Identifier": "component-a" }],
                    },
                },
                "OutputConfig": {
                    "S3OutputLocation": "s3://bucket/benchmarks/all",
                    "MlflowConfig": {
                        "MlflowResourceArn": format!(
                            "arn:aws:sagemaker:us-east-1:{ACCOUNT}:mlflow-app/app-1"
                        ),
                    },
                },
                "NetworkConfig": {
                    "VpcConfig": {
                        "SecurityGroupIds": ["sg-0123456789abcdef0", "sg-1"],
                        "Subnets": ["subnet-0123456789abcdef0"],
                    },
                },
                "Tags": tags(2),
            }),
        },
    ]
}

/// A create request body: the job name plus whatever members the row carries.
fn create_body(name: &str, members: &Value) -> Value {
    let mut body = json!({ "AIBenchmarkJobName": name });
    let object = body.as_object_mut().expect("the body is a JSON object");
    for (member, value) in members.as_object().expect("the row is a JSON object") {
        object.insert(member.clone(), value.clone());
    }
    body
}

/// Create the workload configuration every job here references, in `region`, beside the
/// endpoint their `BenchmarkTarget` names.
async fn seed_workload_config(emulator: &Emulator, region: &str) {
    let (status, body) = emulator
        .call_in(
            region,
            "CreateAIWorkloadConfig",
            &json!({ "AIWorkloadConfigName": WORKLOAD_CONFIG }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case setup region={region}: expected CreateAIWorkloadConfig to answer 200, \
         got {status} with {body:?}"
    );
    seed_target_endpoint(emulator, region, TARGET_ENDPOINT).await;
}

/// Create the endpoint `name` in `region` (with the model and configuration it needs), so a
/// `BenchmarkTarget` naming it resolves.
pub(crate) async fn seed_target_endpoint(emulator: &Emulator, region: &str, name: &str) {    let model = format!("{name}-model");
    let config = format!("{name}-config");
    for (operation, body) in [
        (
            "CreateModel",
            json!({
                "ModelName": model,
                "PrimaryContainer": primary_container(
                    "000000000000.dkr.ecr.us-east-1.amazonaws.com/ranker:1",
                ),
                "ExecutionRoleArn": ROLE_ARN,
            }),
        ),
        ("CreateEndpointConfig", one_variant_config(&config, &model)),
        (
            "CreateEndpoint",
            json!({ "EndpointName": name, "EndpointConfigName": config }),
        ),
    ] {
        let (status, answered) = emulator.call_in(region, operation, &body).await;
        assert_eq!(
            status, 200,
            "case setup region={region}: expected {operation} to answer 200, got {status} \
             with {answered:?}"
        );
    }
}

/// `DescribeAIBenchmarkJob` for `name`, as the raw (status, body) pair, so a test can
/// assert on a refusal as well as on a stored job.
async fn describe(emulator: &Emulator, name: &str) -> (u16, Value) {
    emulator
        .call(
            "DescribeAIBenchmarkJob",
            &json!({ "AIBenchmarkJobName": name }),
        )
        .await
}

/// RULE: a created AI benchmark job is readable back under its own name, and the read
/// replays exactly the members the create stored — nothing invented, nothing dropped —
/// plus the ARN the create answered, the non-terminal status an accepted job starts in
/// and a creation timestamp.
#[tokio::test]
async fn describe_replays_every_member_the_create_stored() {
    for row in round_trip_rows() {
        let emulator = Emulator::new();
        seed_workload_config(&emulator, "us-east-1").await;
        let case = row.case;
        let name = "bench-round-trip";
        let body = create_body(name, &row.members);

        let (status, created) = emulator.call("CreateAIBenchmarkJob", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateAIBenchmarkJob to answer 200, got {status} \
             with {created:?}"
        );
        let arn = str_field(&created, "AIBenchmarkJobArn");
        assert_eq!(
            arn,
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:ai-benchmark-job/{name}"),
            "case {case:?}: expected the minted ARN to name the resource kind and name, \
             got {created:?}"
        );

        let (status, described) = describe(&emulator, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeAIBenchmarkJob to answer 200, got {status} \
             with {described:?}"
        );
        assert_eq!(
            str_field(&described, "AIBenchmarkJobName"),
            name,
            "case {case:?}: expected the stored name, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "AIBenchmarkJobArn"),
            arn,
            "case {case:?}: expected the same ARN the create answered, got {described:?}"
        );
        // An accepted job starts in the one non-terminal member of
        // `AIBenchmarkJobStatus` (smithy_query shape=AIBenchmarkJobStatus: InProgress,
        // Completed, Failed, Stopping, Stopped), so a caller reading straight after the
        // create still observes it.
        assert_eq!(
            str_field(&described, "AIBenchmarkJobStatus"),
            "InProgress",
            "case {case:?}: expected a freshly created job to report InProgress, \
             got {described:?}"
        );
        assert!(
            described.get("StartTime").is_none() && described.get("EndTime").is_none(),
            "case {case:?}: expected no run timestamps before the job ends, got {described:?}"
        );

        // Every member the request carried comes back exactly as sent; one the request
        // omitted is absent, never defaulted to a value a caller cannot distinguish.
        for member in [
            "AIWorkloadConfigIdentifier",
            "RoleArn",
            "BenchmarkTarget",
            "NetworkConfig",
        ] {
            assert_eq!(
                described.get(member),
                body.get(member),
                "case {case:?}: expected {member} to round-trip, got {:?}",
                described.get(member)
            );
        }
        // `OutputConfig` is reported as an `AIBenchmarkOutputResult`, which carries the
        // stored S3 location and MLflow destination plus the log streams of a run.
        let sent_output = body.get("OutputConfig").expect("the row sends OutputConfig");
        let read_output = described
            .get("OutputConfig")
            .unwrap_or_else(|| panic!("case {case:?}: expected an OutputConfig in {described:?}"));
        assert_eq!(
            read_output.get("S3OutputLocation"),
            sent_output.get("S3OutputLocation"),
            "case {case:?}: expected S3OutputLocation to round-trip, got {read_output:?}"
        );
        assert_eq!(
            read_output.get("MlflowConfig"),
            sent_output.get("MlflowConfig"),
            "case {case:?}: expected MlflowConfig to round-trip, got {read_output:?}"
        );

        let sent_tags = body
            .get("Tags")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let read_tags = described
            .get("Tags")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        assert_eq!(
            read_tags, sent_tags,
            "case {case:?}: expected the stored tags to round-trip, got {:?}",
            described.get("Tags")
        );
        // The tag set is the job's own collection, so the member is present whatever is in
        // it: a job created with no tags reports the EMPTY list rather than going missing
        // (the rule the `read_responses_report_what_a_resource_holds` suite pins).
        if sent_tags.is_empty() {
            assert_eq!(
                described.get("Tags"),
                Some(&json!([])),
                "case {case:?}: expected Tags to be the empty list when none were given, \
                 got {:?}",
                described.get("Tags")
            );
        }

        // The creation timestamp is present and stable: a second read reports the same
        // instant, so the read is reporting rather than re-stamping.
        let creation_time = described
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a CreationTime in {described:?}"));
        let (status, again) = describe(&emulator, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the second read to answer 200, got {status} with {again:?}"
        );
        assert_eq!(
            again.get("CreationTime").and_then(Value::as_f64),
            Some(creation_time),
            "case {case:?}: expected CreationTime to be stable across reads, got {:?}",
            again.get("CreationTime")
        );
        assert_eq!(
            again, described,
            "case {case:?}: expected a read to change nothing a later read sees, got {again:?}"
        );
    }
}

/// RULE: the ARN a create mints is
/// `arn:aws:sagemaker:<signed region>:<account>:ai-benchmark-job/<name>`, whatever the
/// name's shape and whichever region the caller signed for (the emulator default when the
/// request is unsigned), and a name outside the modelled constraints never reaches the
/// store.
#[tokio::test]
async fn minted_arns_carry_the_signed_region_and_the_stored_name() {
    // Rows differ along the axes the rule generalises over: the name's shape and the
    // region. `AIBenchmarkJobName` targets `AIEntityName`, `len 1..63` with pattern
    // `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$` (smithy_query shape=AIEntityName), so the rows
    // sit on both published bounds and in between.
    let names = [
        "b",
        "bench1",
        "nightly-latency-benchmark-job",
        "a234567890123456789012345678901234567890123456789012345678901b",
    ];
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];
    let row = &round_trip_rows()[0];

    for region in regions {
        for name in names {
            let emulator = Emulator::new();
            seed_workload_config(&emulator, region).await;
            let case = format!("region={region} name={name}");
            let (status, created) = emulator
                .call_in(region, "CreateAIBenchmarkJob", &create_body(name, &row.members))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateAIBenchmarkJob to answer 200, got {status} \
                 with {created:?}"
            );
            assert_eq!(
                str_field(&created, "AIBenchmarkJobArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:ai-benchmark-job/{name}"),
                "case {case:?}: expected the ARN to carry the signed region and the name, \
                 got {created:?}"
            );
        }
    }

    // A name or role ARN outside the modelled constraints never reaches the store: the
    // boundary validator refuses it, so no job is created for it.
    // `too_long` is one past the published upper bound of `AIEntityName`'s `len 1..63`.
    let too_long = "a".repeat(64);
    let bad_names = ["-leading-hyphen", "under_score", too_long.as_str()];
    for name in bad_names {
        let emulator = Emulator::new();
        seed_workload_config(&emulator, "us-east-1").await;
        let (status, body) = emulator
            .call("CreateAIBenchmarkJob", &create_body(name, &row.members))
            .await;
        assert_eq!(
            status, 400,
            "case name={name:?}: expected a constraint refusal with HTTP 400, got {status} \
             with {body:?}"
        );
    }

    // An unsigned request names no region, so the ARN carries the emulator's default.
    let emulator = Emulator::new();
    let (status, body) = emulator
        .call_unsigned(
            "CreateAIWorkloadConfig",
            &json!({ "AIWorkloadConfigName": WORKLOAD_CONFIG }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case unsigned setup: expected CreateAIWorkloadConfig to answer 200, got {status} \
         with {body:?}"
    );
    // The endpoint store is keyed by `EndpointName` alone, so the target seeded here
    // resolves for the unsigned create too.
    seed_target_endpoint(&emulator, "us-east-1", TARGET_ENDPOINT).await;
    let (status, body) = emulator
        .call_unsigned(
            "CreateAIBenchmarkJob",
            &create_body("unsigned-bench", &row.members),
        )
        .await;
    assert_eq!(
        status, 200,
        "case unsigned: expected CreateAIBenchmarkJob to answer 200, got {status} with {body:?}"
    );
    assert_eq!(
        str_field(&body, "AIBenchmarkJobArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:ai-benchmark-job/unsigned-bench"),
        "case unsigned: expected the ARN to fall back to the default region, got {body:?}"
    );
}

/// RULE: an accepted benchmark job owes exactly one status move to the CLOCK: it reports
/// the non-terminal `InProgress` until the move falls due, then the terminal status with
/// the reason the run ended, an `EndTime` at the instant the move was due and still no
/// `StartTime` — and the terminal report is stable across later reads.
#[tokio::test]
async fn a_job_leaves_in_progress_only_once_the_clock_owes_it() {
    // The move out of `InProgress` does not depend on which optional members the request
    // carried, and each row waits for the due instant — so this rule is exercised over
    // two representative rows (the minimal request and the one carrying every member),
    // while the round-trip and refusal rules above span the whole table.
    let rows = round_trip_rows();
    let clock_rows = [&rows[0], &rows[rows.len() - 1]];
    for row in clock_rows {
        let emulator = Emulator::new();
        seed_workload_config(&emulator, "us-east-1").await;
        let case = row.case;
        let name = "bench-clock";

        let (status, created) = emulator
            .call("CreateAIBenchmarkJob", &create_body(name, &row.members))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateAIBenchmarkJob to answer 200, got {status} \
             with {created:?}"
        );

        // Straight after the create the move is not due yet, so the transient status is
        // observable.
        let (status, before) = describe(&emulator, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first read to answer 200, got {status} with {before:?}"
        );
        assert_eq!(
            str_field(&before, "AIBenchmarkJobStatus"),
            "InProgress",
            "case {case:?}: expected InProgress before the move falls due, got {before:?}"
        );
        assert!(
            before.get("FailureReason").is_none(),
            "case {case:?}: expected no FailureReason while the job is InProgress, \
             got {before:?}"
        );

        tokio::time::sleep(PAST_JOB_END_DUE).await;

        let (status, after) = describe(&emulator, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read after the due instant to answer 200, \
             got {status} with {after:?}"
        );
        let settled = str_field(&after, "AIBenchmarkJobStatus");
        assert_eq!(
            settled, "Failed",
            "case {case:?}: expected the job to end in the modelled Failed status — no \
             benchmark ran here — got {after:?}"
        );
        assert!(
            !str_field(&after, "FailureReason").is_empty(),
            "case {case:?}: expected a FailureReason beside the terminal status, got {after:?}"
        );
        let end_time = after
            .get("EndTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected an EndTime in {after:?}"));
        let creation_time = after
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a CreationTime in {after:?}"));
        assert!(
            end_time >= creation_time,
            "case {case:?}: expected EndTime {end_time} at or after CreationTime \
             {creation_time}, got {after:?}"
        );
        assert!(
            after.get("StartTime").is_none(),
            "case {case:?}: expected no StartTime for a run that never started, got {after:?}"
        );

        // The move happened once: a later read reports exactly the same job, so the
        // terminal status and its timestamps are stable and no read re-stamps them.
        let (status, again) = describe(&emulator, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the repeated read to answer 200, got {status} \
             with {again:?}"
        );
        assert_eq!(
            again, after,
            "case {case:?}: expected the settled job to be stable across reads, got {again:?}"
        );
    }
}

/// RULE: `AIBenchmarkJobName` is unique per account and Region, so a create naming one
/// that already exists is refused with `ResourceInUse` and leaves the stored job exactly
/// as the first create left it.
#[tokio::test]
async fn a_name_already_in_use_is_refused_and_the_stored_job_is_untouched() {
    for row in round_trip_rows() {
        let emulator = Emulator::new();
        seed_workload_config(&emulator, "us-east-1").await;
        let case = row.case;
        let name = "bench-duplicate";

        let (status, first) = emulator
            .call("CreateAIBenchmarkJob", &create_body(name, &row.members))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first create to answer 200, got {status} with {first:?}"
        );
        let (status, before) = describe(&emulator, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first read to answer 200, got {status} with {before:?}"
        );

        // A second create under the same name, carrying DIFFERENT members, must not
        // overwrite the first.
        let (status, refused) = emulator
            .call(
                "CreateAIBenchmarkJob",
                &json!({
                    "AIBenchmarkJobName": name,
                    "AIWorkloadConfigIdentifier": WORKLOAD_CONFIG,
                    "RoleArn": "arn:aws:iam::000000000000:role/OtherBenchmarkRole",
                    "BenchmarkTarget": { "Endpoint": { "Identifier": "other-endpoint" } },
                    "OutputConfig": { "S3OutputLocation": "s3://other-bucket/out/" },
                    "Tags": tags(1),
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the duplicate create to answer 400, got {status} \
             with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceInUse",
            "case {case:?}: expected the modelled ResourceInUse code, got {refused:?}"
        );

        let (status, after) = describe(&emulator, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read after the refusal to answer 200, got {status} \
             with {after:?}"
        );
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused create to change nothing, got {after:?}"
        );
    }
}

/// RULE: a benchmark job runs the workload its referenced AI workload configuration
/// defines, so a create naming a configuration that does not exist is refused with
/// `ResourceNotFound` and stores nothing — whichever form (name or ARN) named it.
#[tokio::test]
async fn a_job_referencing_an_unknown_workload_configuration_is_refused() {
    let emulator = Emulator::new();
    seed_workload_config(&emulator, "us-east-1").await;

    // Rows differ along the ways a reference can miss: never created, a prefix of one
    // that was, a case variant (names are matched exactly), and the ARN form of each.
    let bare = ["absent-config", "bench", "BENCH-WORKLOAD-CONFIG"];
    let mut identifiers: Vec<String> = bare.iter().map(|name| name.to_string()).collect();
    identifiers.extend(bare.iter().map(|name| {
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:ai-workload-config/{name}")
    }));

    for (index, identifier) in identifiers.iter().enumerate() {
        let name = format!("bench-missing-ref-{index}");
        let (status, body) = emulator
            .call(
                "CreateAIBenchmarkJob",
                &json!({
                    "AIBenchmarkJobName": name,
                    "AIWorkloadConfigIdentifier": identifier,
                    "RoleArn": ROLE_ARN,
                    "BenchmarkTarget": { "Endpoint": { "Identifier": "llama-endpoint" } },
                    "OutputConfig": { "S3OutputLocation": "s3://bucket/benchmarks/" },
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case identifier={identifier:?}: expected HTTP 400 for a workload \
             configuration that does not exist, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFound",
            "case identifier={identifier:?}: expected the modelled ResourceNotFound code, \
             got {body:?}"
        );
        // The refused create stored nothing, so the job name is still free to read-miss.
        let (status, read) = describe(&emulator, &name).await;
        assert_eq!(
            status, 400,
            "case identifier={identifier:?}: expected the refused job to be absent, \
             got {status} with {read:?}"
        );
    }

    // And a job naming the configuration that WAS created is accepted, so the refusals
    // above are about the reference and not about a create that refuses everything.
    let (status, body) = emulator
        .call(
            "CreateAIBenchmarkJob",
            &json!({
                "AIBenchmarkJobName": "bench-present-ref",
                "AIWorkloadConfigIdentifier": WORKLOAD_CONFIG,
                "RoleArn": ROLE_ARN,
                "BenchmarkTarget": { "Endpoint": { "Identifier": "llama-endpoint" } },
                "OutputConfig": { "S3OutputLocation": "s3://bucket/benchmarks/" },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case identifier={WORKLOAD_CONFIG:?}: expected HTTP 200 for the configuration that \
         exists, got {status} with {body:?}"
    );
}

/// RULE: a read of an AI benchmark job that does not exist is refused with
/// `ResourceNotFound`, never answered with a 200 carrying empty members — and distinct
/// names are distinct jobs, each reporting its own ARN and members.
#[tokio::test]
async fn distinct_names_are_distinct_jobs_and_an_unknown_name_is_refused() {
    let emulator = Emulator::new();
    seed_workload_config(&emulator, "us-east-1").await;
    let rows = round_trip_rows();

    for (index, row) in rows.iter().enumerate() {
        let name = format!("bench{index}");
        let (status, created) = emulator
            .call("CreateAIBenchmarkJob", &create_body(&name, &row.members))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected create {name} to answer 200, got {status} with {created:?}",
            row.case
        );
    }

    for (index, row) in rows.iter().enumerate() {
        let name = format!("bench{index}");
        let (status, described) = describe(&emulator, &name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected read {name} to answer 200, got {status} with {described:?}",
            row.case
        );
        assert_eq!(
            str_field(&described, "AIBenchmarkJobArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:ai-benchmark-job/{name}"),
            "case {:?}: expected {name}'s own ARN, got {described:?}",
            row.case
        );
        let expected = create_body(&name, &row.members);
        for member in ["BenchmarkTarget", "RoleArn", "NetworkConfig"] {
            assert_eq!(
                described.get(member),
                expected.get(member),
                "case {:?}: expected {name}'s own {member}, got {:?}",
                row.case,
                described.get(member)
            );
        }
    }

    // Rows differ along the ways a name can miss: never created, a prefix of one that
    // was, a suffix, and a case variant.
    for name in ["absent", "bench", "ench0", "BENCH0", "bench00"] {
        let (status, body) = describe(&emulator, name).await;
        assert_eq!(
            status, 400,
            "case name={name:?}: expected HTTP 400 for a name that was never created, \
             got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFound",
            "case name={name:?}: expected the modelled ResourceNotFound code, got {body:?}"
        );
    }
}
