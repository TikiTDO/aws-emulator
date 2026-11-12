// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the members SageMaker itself supplies, as the read paths
//! report them: the documented default of an optional member the create omitted, the
//! value the service resolves (a container image's digest), the identity a call was
//! signed with, and a collection reported EMPTY when the resource holds nothing.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the harness `tests/all/inference_deployment_lifecycle.rs` owns,
//! so what these tests exercise is what a real SDK client gets.
//!
//! The rule under test, over a TABLE of inputs rather than one recorded call: a resource
//! is stored MATERIALISED — the create resolves what the service supplies — so a reader
//! replays the whole resource rather than only the members the client happened to send,
//! and the value for a member the request omitted is the one the model documents.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_sagemaker_sagemaker::state;

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// Long enough to pass `state::FEATURE_GROUP_CREATED_AFTER` (300 ms).
const PAST_CREATED_DUE: Duration = Duration::from_millis(600);

/// How long a poll below gives the clock to settle a training job: far longer than the span
/// the state gives a job's work, so a loaded host still settles well inside it.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How often such a poll looks again. Only the loop's condition decides when it stops.
const SETTLE_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The type prefix an IAM user's unique id carries, from IAM's documented "unique
/// identifier prefixes" table (`AIDA` for a user).
const IAM_USER_ID_PREFIX: &str = "AIDA";

/// The `UserContext` every read reports for a resource created through this harness,
/// which signs with `Credential=AKIDEXAMPLE/...`: the IAM identity behind the call, as
/// its ARN plus the unique id AWS gives that identity.
pub(crate) fn expected_caller_identity() -> Value {
    let arn = format!("arn:aws:iam::{ACCOUNT}:user/AKIDEXAMPLE");
    let principal_id = service_common::caller_principal::aws_unique_id(IAM_USER_ID_PREFIX, &arn);
    json!({ "IamIdentity": { "Arn": arn, "PrincipalId": principal_id } })
}

/// `specification` with the members the service resolves for an algorithm's training
/// specification laid over it: the digest of the training image, and each training
/// channel's `IsRequired` where the request stated none.
///
/// Shared with `algorithm_lifecycle`, which round-trips the same shape.
pub(crate) fn materialised_training_specification(specification: &Value) -> Value {
    let mut expected = specification.clone();
    if expected.get("TrainingImageDigest").is_none() {
        let image = expected
            .get("TrainingImage")
            .and_then(Value::as_str)
            .expect("a training specification names its image");
        expected["TrainingImageDigest"] = json!(state::derived_image_digest(image));
    }
    for channel in expected
        .get_mut("TrainingChannels")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if channel.get("IsRequired").is_none() {
            channel["IsRequired"] = json!(false);
        }
    }
    expected
}

/// `specification` with the members the service resolves for an inference
/// specification's containers laid over it: each container's image digest, and its
/// `IsCheckpoint` where the request stated none.
///
/// Shared with `algorithm_lifecycle` and `model_package_updates`, which round-trip the
/// same shape.
pub(crate) fn materialised_inference_specification(specification: &Value) -> Value {
    let mut expected = specification.clone();
    for container in expected
        .get_mut("Containers")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if container.get("ImageDigest").is_none() {
            if let Some(image) = container.get("Image").and_then(Value::as_str) {
                container["ImageDigest"] = json!(state::derived_image_digest(image));
            }
        }
        if container.get("IsCheckpoint").is_none() {
            container["IsCheckpoint"] = json!(false);
        }
    }
    expected
}

/// `specifications`, a list of additional inference specifications, with the members the
/// service resolves for each one's containers laid over it.
pub(crate) fn materialised_additional_inference_specifications(specifications: &Value) -> Value {
    match specifications.as_array() {
        Some(listed) => Value::Array(
            listed
                .iter()
                .map(materialised_inference_specification)
                .collect(),
        ),
        None => specifications.clone(),
    }
}

/// A `TrainingSpecification` naming `image`, with `channels` channels; the channel at
/// index 0 states `IsRequired` only when `state_required` is set, so a row can carry both
/// a stated and an unstated channel.
fn training_specification(image: &str, channels: usize, state_required: Option<bool>) -> Value {
    let mut listed = Vec::new();
    for index in 0..channels {
        let mut channel = json!({
            "Name": format!("channel{index}"),
            "SupportedContentTypes": ["text/csv"],
            "SupportedInputModes": ["File"],
        });
        if index == 0 {
            if let Some(required) = state_required {
                channel["IsRequired"] = json!(required);
            }
        }
        listed.push(channel);
    }
    json!({
        "TrainingImage": image,
        "SupportedTrainingInstanceTypes": ["ml.m5.large"],
        "TrainingChannels": listed,
    })
}

/// An `InferenceSpecification` whose containers are `images`.
///
/// A model package's container image has to be one of the two formats its member documents
/// (`registry/repository[:tag]` / `registry/repository[@digest]`), so the rows below name a
/// repository behind this account's own Amazon ECR registry; the registry holds none of
/// them, which registration does not ask about.
fn inference_specification(images: &[&str]) -> Value {
    let containers: Vec<Value> = images
        .iter()
        .map(|image| json!({ "Image": image, "ModelDataUrl": "s3://models/artifact.tar.gz" }))
        .collect();
    json!({
        "Containers": containers,
        "SupportedContentTypes": ["text/csv"],
        "SupportedTransformInstanceTypes": ["ml.m5.large"],
        "SupportedRealtimeInferenceInstanceTypes": ["ml.c5.xlarge"],
    })
}

/// RULE: the digest a read reports for a container image is derived from that image
/// reference and from nothing else — the modelled `sha256:<64 hex digits>` form, the same
/// for one reference however often it is read, and different for different references.
#[tokio::test]
async fn image_digests_are_derived_from_the_image_reference_and_are_stable() {
    // Rows differ along the image reference's shape: a bare repository, a tag, a registry
    // path, a digest-style reference and a non-ASCII repository name.
    let images = [
        "trainer",
        "trainer:1",
        "trainer:2",
        &format!("{ACCOUNT}.dkr.ecr.us-east-1.amazonaws.com/trainer:latest"),
        "registry.example.com/team/trainer@sha256:cafe",
        "régistre/entraîneur:1",
    ];

    let emulator = Emulator::new();
    let mut seen: Vec<(String, String)> = Vec::new();
    for (index, image) in images.iter().enumerate() {
        let name = format!("digest{index}");
        let case = format!("image={image}");
        emulator
            .ok(
                "CreateAlgorithm",
                &json!({
                    "AlgorithmName": name,
                    "TrainingSpecification": training_specification(image, 1, None),
                    "InferenceSpecification": inference_specification(&[image]),
                }),
            )
            .await;
        let described = emulator
            .ok("DescribeAlgorithm", &json!({ "AlgorithmName": name }))
            .await;

        let digest = described
            .pointer("/TrainingSpecification/TrainingImageDigest")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let expected = state::derived_image_digest(image);
        assert_eq!(
            digest, expected,
            "case {case}: TrainingImageDigest must be the digest resolved for the image, \
             got {described:?}"
        );
        let (prefix, hex) = digest
            .split_once(':')
            .unwrap_or_else(|| panic!("case {case}: expected a <algorithm>:<hex> digest, got {digest:?}"));
        // `ImageDigest`'s modelled pattern is `^[Ss][Hh][Aa]256:[0-9a-fA-F]{64}$`.
        assert_eq!(
            prefix, "sha256",
            "case {case}: expected the sha256 digest algorithm, got {digest:?}"
        );
        assert_eq!(
            hex.len(),
            64,
            "case {case}: expected 64 hex digits, got {digest:?}"
        );
        assert!(
            hex.chars().all(|digit| digit.is_ascii_hexdigit()),
            "case {case}: expected hex digits, got {digest:?}"
        );
        // The inference container's digest is resolved from the same reference, so the
        // two agree.
        assert_eq!(
            described
                .pointer("/InferenceSpecification/Containers/0/ImageDigest")
                .and_then(Value::as_str),
            Some(digest.as_str()),
            "case {case}: an inference container's ImageDigest, got {described:?}"
        );
        // A second read reports the same digest: it is stored, not re-derived per read.
        let reread = emulator
            .ok("DescribeAlgorithm", &json!({ "AlgorithmName": name }))
            .await;
        assert_eq!(
            reread.pointer("/TrainingSpecification/TrainingImageDigest"),
            described.pointer("/TrainingSpecification/TrainingImageDigest"),
            "case {case}: the digest must be stable across reads, got {reread:?}"
        );

        for (held_image, held_digest) in &seen {
            if held_image != image {
                assert_ne!(
                    held_digest, &digest,
                    "case {case}: {held_image:?} and {image:?} must not share a digest"
                );
            }
        }
        seen.push(((*image).to_string(), digest));
    }
}

/// RULE: `DescribeAlgorithm` reports the certification flag and each training channel's
/// requirement for every algorithm — the value the request stated, or the documented
/// `false` when it stated none.
#[tokio::test]
async fn algorithm_reads_report_the_booleans_the_service_defaults() {
    // Rows differ along what the create states (nothing, each boolean explicitly) and
    // along the channel count, which is what the per-channel default is applied over.
    let rows: Vec<(&str, Option<bool>, Option<bool>, usize)> = vec![
        ("nothing-stated", None, None, 1),
        ("certified", Some(true), None, 1),
        ("certification-off", Some(false), None, 2),
        ("channel-required", None, Some(true), 2),
        ("channel-not-required", None, Some(false), 3),
        ("both-stated", Some(true), Some(true), 8),
    ];

    for (case, certify, required, channels) in rows {
        let emulator = Emulator::new();
        let specification = training_specification("trainer:1", channels, required);
        let mut body = json!({
            "AlgorithmName": case,
            "TrainingSpecification": specification.clone(),
        });
        if let Some(certify) = certify {
            body["CertifyForMarketplace"] = json!(certify);
        }
        emulator.ok("CreateAlgorithm", &body).await;

        let described = emulator
            .ok("DescribeAlgorithm", &json!({ "AlgorithmName": case }))
            .await;
        assert_eq!(
            described.get("CertifyForMarketplace").and_then(Value::as_bool),
            Some(certify.unwrap_or(false)),
            "case {case}: CertifyForMarketplace, got {described:?}"
        );
        assert_eq!(
            described.get("TrainingSpecification"),
            Some(&materialised_training_specification(&specification)),
            "case {case}: TrainingSpecification, got {described:?}"
        );
        let reported: Vec<Option<bool>> = described
            .pointer("/TrainingSpecification/TrainingChannels")
            .and_then(Value::as_array)
            .map(|listed| {
                listed
                    .iter()
                    .map(|channel| channel.get("IsRequired").and_then(Value::as_bool))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            reported.len(),
            channels,
            "case {case}: every channel must be reported, got {described:?}"
        );
        assert!(
            reported.iter().all(Option::is_some),
            "case {case}: every channel reports its IsRequired, got {described:?}"
        );
    }
}

/// RULE: `DescribeModel` reports how each container hosts its models and whether the
/// model is network-isolated — the value the request stated, or the documented default
/// (`SingleModel`, `false`) when it stated none.
#[tokio::test]
async fn model_reads_report_the_container_mode_and_isolation_the_service_defaults() {
    // Rows differ along the container count (primary only, primary plus a pipeline), what
    // the request states for `Mode`, and what it states for `EnableNetworkIsolation`.
    let rows: Vec<(&str, Option<&str>, Option<bool>, usize)> = vec![
        ("nothing-stated", None, None, 0),
        ("isolated", None, Some(true), 0),
        ("isolation-off", None, Some(false), 0),
        ("multi-model", Some("MultiModel"), None, 0),
        ("single-model-stated", Some("SingleModel"), None, 0),
        ("inference-pipeline", None, None, 2),
        ("pipeline-multi-model", Some("MultiModel"), Some(true), 3),
    ];

    for (case, mode, isolation, pipeline) in rows {
        let emulator = Emulator::new();
        let mut primary = json!({ "Image": "inference:1" });
        if let Some(mode) = mode {
            primary["Mode"] = json!(mode);
        }
        let mut body = json!({ "ModelName": case, "PrimaryContainer": primary });
        if let Some(isolation) = isolation {
            body["EnableNetworkIsolation"] = json!(isolation);
        }
        if pipeline > 0 {
            let containers: Vec<Value> = (0..pipeline)
                .map(|index| {
                    let mut container = json!({
                        "Image": format!("inference:{index}"),
                        "ContainerHostname": format!("step{index}"),
                    });
                    if let Some(mode) = mode {
                        container["Mode"] = json!(mode);
                    }
                    container
                })
                .collect();
            body["Containers"] = json!(containers);
        }
        emulator.ok("CreateModel", &body).await;

        let described = emulator
            .ok("DescribeModel", &json!({ "ModelName": case }))
            .await;
        assert_eq!(
            described.get("EnableNetworkIsolation").and_then(Value::as_bool),
            Some(isolation.unwrap_or(false)),
            "case {case}: EnableNetworkIsolation, got {described:?}"
        );
        let expected_mode = mode.unwrap_or("SingleModel");
        assert_eq!(
            described
                .pointer("/PrimaryContainer/Mode")
                .and_then(Value::as_str),
            Some(expected_mode),
            "case {case}: PrimaryContainer.Mode, got {described:?}"
        );
        let modes: Vec<Option<&str>> = described
            .get("Containers")
            .and_then(Value::as_array)
            .map(|listed| {
                listed
                    .iter()
                    .map(|container| container.get("Mode").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            modes.len(),
            pipeline,
            "case {case}: every pipeline container must be reported, got {described:?}"
        );
        assert!(
            modes.iter().all(|reported| *reported == Some(expected_mode)),
            "case {case}: every pipeline container reports its Mode, got {described:?}"
        );
        // The image itself is echoed unchanged beside the resolved member.
        assert_eq!(
            described
                .pointer("/PrimaryContainer/Image")
                .and_then(Value::as_str),
            Some("inference:1"),
            "case {case}: PrimaryContainer.Image, got {described:?}"
        );
    }
}

/// RULE: `DescribeEndpointConfig` reports the utilization-metrics configuration and the
/// isolation flag the endpoint would RUN under — the value the request stated, or the
/// documented default (`EnableDetailedObservability` true for a new configuration,
/// `false` for isolation) when it stated none.
#[tokio::test]
async fn endpoint_config_reads_report_the_metrics_and_isolation_the_service_resolves() {
    // Rows differ along what the request states: neither member, each on its own, and a
    // `MetricsConfig` naming only one of its OTHER members
    // (`smithy_query(query="shape",shape="MetricsConfig")` lists `EnableEnhancedMetrics`,
    // `EnableDetailedObservability` and `MetricPublishFrequencyInSeconds`), which leaves
    // the defaulted one to the service.
    let rows: Vec<(&str, Option<bool>, Option<Value>)> = vec![
        ("nothing-stated", None, None),
        ("isolated", Some(true), None),
        ("isolation-off", Some(false), None),
        (
            "observability-off",
            None,
            Some(json!({ "EnableDetailedObservability": false })),
        ),
        (
            "observability-on",
            Some(true),
            Some(json!({ "EnableDetailedObservability": true })),
        ),
        (
            "enhanced-only",
            None,
            Some(json!({ "EnableEnhancedMetrics": true })),
        ),
        (
            // `MetricPublishFrequencyInSeconds` is an intEnum whose values are 10, 30, 60,
            // 120, 180, 240 and 300 (the member's own documentation).
            "frequency-only",
            None,
            Some(json!({ "MetricPublishFrequencyInSeconds": 30 })),
        ),
    ];

    for (case, isolation, metrics) in rows {
        let emulator = Emulator::new();
        let mut body = json!({
            "EndpointConfigName": case,
            "ProductionVariants": [{
                "VariantName": "AllTraffic",
                "InitialInstanceCount": 1,
                "InstanceType": "ml.m5.large",
                "InitialVariantWeight": 1.0,
            }],
        });
        if let Some(isolation) = isolation {
            body["EnableNetworkIsolation"] = json!(isolation);
        }
        if let Some(metrics) = &metrics {
            body["MetricsConfig"] = metrics.clone();
        }
        emulator.ok("CreateEndpointConfig", &body).await;

        let described = emulator
            .ok(
                "DescribeEndpointConfig",
                &json!({ "EndpointConfigName": case }),
            )
            .await;
        assert_eq!(
            described
                .get("EnableNetworkIsolation")
                .and_then(Value::as_bool),
            Some(isolation.unwrap_or(false)),
            "case {case}: EnableNetworkIsolation, got {described:?}"
        );
        let stated_observability = metrics
            .as_ref()
            .and_then(|metrics| metrics.get("EnableDetailedObservability"))
            .and_then(Value::as_bool);
        assert_eq!(
            described
                .pointer("/MetricsConfig/EnableDetailedObservability")
                .and_then(Value::as_bool),
            Some(stated_observability.unwrap_or(true)),
            "case {case}: MetricsConfig.EnableDetailedObservability, got {described:?}"
        );
        // Every other metrics member is echoed exactly as stated beside the resolved one,
        // and stays absent where the request stated none.
        for member in ["EnableEnhancedMetrics", "MetricPublishFrequencyInSeconds"] {
            assert_eq!(
                described
                    .pointer(&format!("/MetricsConfig/{member}")),
                metrics.as_ref().and_then(|metrics| metrics.get(member)),
                "case {case}: MetricsConfig.{member}, got {described:?}"
            );
        }
    }
}

/// Register the model package group `group` so a versioned package can join it.
async fn create_group(emulator: &Emulator, group: &str) {
    emulator
        .ok(
            "CreateModelPackageGroup",
            &json!({ "ModelPackageGroupName": group }),
        )
        .await;
}

/// One row of the model-package read table: the case (and the package's name or group),
/// whether the package is a version inside a group, the container images its inference
/// specification carries, what the request states for the certification flag, and what it
/// states for the registration type.
struct PackageRow {
    case: &'static str,
    versioned: bool,
    images: &'static [&'static str],
    certify: Option<bool>,
    registration: Option<&'static str>,
}

/// RULE: `DescribeModelPackage` reports the registry's own members for every package —
/// the registration type, the certification flag, the image scan statuses it holds (none,
/// so the EMPTY list) and the identity that registered it — beside the containers it
/// stored with their resolved digests.
#[tokio::test]
async fn model_package_reads_report_the_members_the_registry_supplies() {
    // Rows differ along versioning (a version in a group, an unversioned package), the
    // container count, whether the request states the certification flag and whether it
    // states the registration type.
    let rows = [
        PackageRow {
            case: "versioned-defaults",
            versioned: true,
            images: &["000000000000.dkr.ecr.us-east-1.amazonaws.com/inference:1"],
            certify: None,
            registration: None,
        },
        PackageRow {
            case: "versioned-certified",
            versioned: true,
            images: &["000000000000.dkr.ecr.us-east-1.amazonaws.com/inference:1"],
            certify: Some(true),
            registration: None,
        },
        PackageRow {
            case: "versioned-two-containers",
            versioned: true,
            images: &[
                "000000000000.dkr.ecr.us-east-1.amazonaws.com/inference:1",
                "000000000000.dkr.ecr.us-east-1.amazonaws.com/inference:2",
            ],
            certify: Some(false),
            registration: None,
        },
        PackageRow {
            case: "versioned-logged",
            versioned: true,
            images: &["000000000000.dkr.ecr.us-east-1.amazonaws.com/inference:1"],
            certify: None,
            registration: Some("Logged"),
        },
        PackageRow {
            case: "unversioned-defaults",
            versioned: false,
            images: &["000000000000.dkr.ecr.us-east-1.amazonaws.com/inference:9"],
            certify: None,
            registration: None,
        },
        PackageRow {
            case: "unversioned-three-containers",
            versioned: false,
            images: &[
                "000000000000.dkr.ecr.us-east-1.amazonaws.com/a:1",
                "000000000000.dkr.ecr.us-east-1.amazonaws.com/b:2",
                "000000000000.dkr.ecr.us-east-1.amazonaws.com/c:3",
            ],
            certify: None,
            registration: Some("Registered"),
        },
    ];

    for PackageRow {
        case,
        versioned,
        images,
        certify,
        registration,
    } in rows
    {
        let emulator = Emulator::new();
        let specification = inference_specification(images);
        let mut body = json!({ "InferenceSpecification": specification.clone() });
        if versioned {
            create_group(&emulator, case).await;
            body["ModelPackageGroupName"] = json!(case);
        } else {
            body["ModelPackageName"] = json!(case);
        }
        if let Some(certify) = certify {
            body["CertifyForMarketplace"] = json!(certify);
        }
        if let Some(registration) = registration {
            body["ModelPackageRegistrationType"] = json!(registration);
        }
        let created = emulator.ok("CreateModelPackage", &body).await;
        let arn = str_field(&created, "ModelPackageArn");

        let described = emulator
            .ok("DescribeModelPackage", &json!({ "ModelPackageName": arn }))
            .await;
        assert_eq!(
            described.get("CertifyForMarketplace").and_then(Value::as_bool),
            Some(certify.unwrap_or(false)),
            "case {case}: CertifyForMarketplace, got {described:?}"
        );
        assert_eq!(
            described
                .get("ModelPackageRegistrationType")
                .and_then(Value::as_str),
            Some(registration.unwrap_or("Registered")),
            "case {case}: ModelPackageRegistrationType, got {described:?}"
        );
        // The package holds no image scan, so the member reports the EMPTY list rather
        // than going missing — a caller cannot tell a missing member from "nothing held".
        assert_eq!(
            described.pointer("/ModelPackageStatusDetails/ImageScanStatuses"),
            Some(&json!([])),
            "case {case}: ModelPackageStatusDetails.ImageScanStatuses, got {described:?}"
        );
        assert_eq!(
            described.get("InferenceSpecification"),
            Some(&materialised_inference_specification(&specification)),
            "case {case}: InferenceSpecification, got {described:?}"
        );
        assert_eq!(
            described.get("CreatedBy"),
            Some(&expected_caller_identity()),
            "case {case}: CreatedBy, got {described:?}"
        );
    }
}

/// RULE: a model package's summary is served for every version this account holds,
/// keyed by the ARN the request asked about — including one registered from a source
/// algorithm, which declares no inference containers of its own.
#[tokio::test]
async fn batch_describe_summarises_a_version_registered_from_a_source_algorithm() {
    // Rows differ along where the package's inference containers come from: its own
    // specification, a source algorithm this account holds (whose specification applies),
    // and a source algorithm it does not hold.
    let rows: Vec<(&str, bool, bool)> = vec![
        ("own-containers", true, false),
        ("source-algorithm-held", false, true),
        ("source-algorithm-absent", false, false),
    ];

    for (case, own_containers, register_algorithm) in rows {
        let emulator = Emulator::new();
        create_group(&emulator, case).await;
        let algorithm = format!("{case}-algorithm");
        if register_algorithm {
            emulator
                .ok(
                    "CreateAlgorithm",
                    &json!({
                        "AlgorithmName": algorithm,
                        "TrainingSpecification": training_specification("trainer:1", 1, None),
                        "InferenceSpecification": inference_specification(
                            &["000000000000.dkr.ecr.us-east-1.amazonaws.com/inference:7"],
                        ),
                    }),
                )
                .await;
        }
        let mut body = json!({
            "ModelPackageGroupName": case,
            "ModelPackageDescription": "описание — a non-ASCII description",
        });
        if own_containers {
            body["InferenceSpecification"] = inference_specification(
                &["000000000000.dkr.ecr.us-east-1.amazonaws.com/inference:1"],
            );
        } else {
            body["SourceAlgorithmSpecification"] = json!({
                "SourceAlgorithms": [{
                    "AlgorithmName": algorithm,
                    "ModelDataUrl": "s3://models/artifact.tar.gz",
                }],
            });
        }
        let created = emulator.ok("CreateModelPackage", &body).await;
        let arn = str_field(&created, "ModelPackageArn");

        let batch = emulator
            .ok(
                "BatchDescribeModelPackage",
                &json!({ "ModelPackageArnList": [arn] }),
            )
            .await;
        assert_eq!(
            batch.pointer("/BatchDescribeModelPackageErrorMap"),
            Some(&json!({})),
            "case {case}: a version this account holds is summarised, not errored, \
             got {batch:?}"
        );
        let summary = batch
            .get("ModelPackageSummaries")
            .and_then(|summaries| summaries.get(&arn))
            .unwrap_or_else(|| panic!("case {case}: expected a summary for {arn}, got {batch}"));
        assert_eq!(
            summary.get("ModelPackageArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case}: ModelPackageArn, got {summary:?}"
        );
        assert_eq!(
            summary.get("ModelPackageGroupName").and_then(Value::as_str),
            Some(case),
            "case {case}: ModelPackageGroupName, got {summary:?}"
        );
        assert_eq!(
            summary.get("ModelPackageVersion").and_then(Value::as_i64),
            Some(1),
            "case {case}: ModelPackageVersion, got {summary:?}"
        );
        assert_eq!(
            summary.get("ModelPackageDescription").and_then(Value::as_str),
            Some("описание — a non-ASCII description"),
            "case {case}: the description must be preserved byte for byte, got {summary:?}"
        );
        assert_eq!(
            summary
                .get("ModelPackageRegistrationType")
                .and_then(Value::as_str),
            Some("Registered"),
            "case {case}: ModelPackageRegistrationType, got {summary:?}"
        );
        assert!(
            summary.get("CreationTime").and_then(Value::as_f64).is_some(),
            "case {case}: CreationTime, got {summary:?}"
        );
        // The containers a summary reports are the ones that apply to the package: its
        // own, or the source algorithm's where this account holds that algorithm. A
        // package with neither declares no inference container, and the summary says so
        // — `InferenceSpecification` is a member the model marks required of a summary,
        // so the batch read reports the package with an empty container list rather than
        // withholding the whole summary over containers nothing declared.
        let expected_images: Vec<&str> = if own_containers {
            vec!["000000000000.dkr.ecr.us-east-1.amazonaws.com/inference:1"]
        } else if register_algorithm {
            vec!["000000000000.dkr.ecr.us-east-1.amazonaws.com/inference:7"]
        } else {
            Vec::new()
        };
        let reported_images: Vec<String> = summary
            .pointer("/InferenceSpecification/Containers")
            .and_then(Value::as_array)
            .map(|containers| {
                containers
                    .iter()
                    .map(|container| {
                        container
                            .get("Image")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            reported_images, expected_images,
            "case {case}: the summary's inference containers, got {summary:?}"
        );
    }
}

/// RULE: `DescribePipeline` reports the name the pipeline is displayed under — the one
/// the create stated, or its `PipelineName` when it stated none — and the identities
/// behind the create and the last change to it.
#[tokio::test]
async fn pipeline_reads_report_the_display_name_and_the_identities() {
    // Rows differ along whether the create states a display name and along the pipeline
    // name's shape, which is what the default falls back to.
    let rows: Vec<(&str, Option<&str>)> = vec![
        ("p", None),
        ("pipeline-with-hyphens", None),
        ("Pipeline9", Some("Shown-To-Humans")),
        ("display-name-equals-name", Some("display-name-equals-name")),
    ];

    for (name, display) in rows {
        let emulator = Emulator::new();
        let mut body = json!({
            "PipelineName": name,
            "ClientRequestToken": format!("{name}-token-0000000000000000000000000000000"),
            "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/SageMakerPipelineRole"),
            "PipelineDefinition": crate::pipeline_lifecycle::definition(json!([
                crate::pipeline_lifecycle::step("Only", &[]),
            ])),
        });
        if let Some(display) = display {
            body["PipelineDisplayName"] = json!(display);
        }
        emulator.ok("CreatePipeline", &body).await;

        let described = emulator
            .ok("DescribePipeline", &json!({ "PipelineName": name }))
            .await;
        assert_eq!(
            described.get("PipelineDisplayName").and_then(Value::as_str),
            Some(display.unwrap_or(name)),
            "case {name}: PipelineDisplayName, got {described:?}"
        );
        for member in ["CreatedBy", "LastModifiedBy"] {
            assert_eq!(
                described.get(member),
                Some(&expected_caller_identity()),
                "case {name}: {member}, got {described:?}"
            );
        }
        // A listing projects the same display name the reader reports.
        let listed = emulator.ok("ListPipelines", &json!({})).await;
        assert_eq!(
            listed
                .pointer("/PipelineSummaries/0/PipelineDisplayName")
                .and_then(Value::as_str),
            Some(display.unwrap_or(name)),
            "case {name}: the summary's PipelineDisplayName, got {listed:?}"
        );
    }
}

/// A `CreateTrainingJob` body naming `name`, with `channels` input channels and `extra`
/// laid over it.
fn training_body(name: &str, channels: usize, extra: Value) -> Value {
    let listed: Vec<Value> = (0..channels)
        .map(|index| {
            json!({
                "ChannelName": format!("channel{index}"),
                "DataSource": {
                    "S3DataSource": {
                        "S3DataType": "S3Prefix",
                        "S3Uri": format!("s3://demo/{index}/"),
                    },
                },
                "ContentType": "text/csv",
            })
        })
        .collect();
    let mut body = json!({
        "TrainingJobName": name,
        "AlgorithmSpecification": {
            "TrainingImage": "trainer:1",
            "TrainingInputMode": "File",
        },
        "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/SageMakerTrainingRole"),
        "InputDataConfig": listed,
        "OutputDataConfig": { "S3OutputPath": "s3://demo/output/" },
        "ResourceConfig": {
            "InstanceType": "ml.m5.large",
            "InstanceCount": 1,
            "VolumeSizeInGB": 10,
        },
        "StoppingCondition": { "MaxRuntimeInSeconds": 3600 },
    });
    let object = body.as_object_mut().expect("a JSON object body");
    for (key, value) in extra.as_object().expect("a JSON object of extra members") {
        object.insert(key.clone(), value.clone());
    }
    body
}

/// RULE: `DescribeTrainingJob` reports the job as the service accepted it — every input
/// channel echoed with the compression, record wrapping and S3 distribution it runs
/// under, the output configuration with its compression and key, the three switches a
/// job that asked for none of them runs without, and the profiling status its
/// configuration implies.
#[tokio::test]
async fn training_job_reads_report_the_configuration_the_service_resolves() {
    // Rows differ along the channel count, whether the request states the per-channel
    // members, what it states for the output configuration, and the three switches.
    let rows: Vec<(&str, usize, Value)> = vec![
        ("defaults", 1, json!({})),
        ("two-channels", 2, json!({})),
        (
            "channel-members-stated",
            1,
            json!({
                "InputDataConfig": [{
                    "ChannelName": "stated",
                    "DataSource": {
                        "S3DataSource": {
                            "S3DataType": "S3Prefix",
                            "S3Uri": "s3://demo/stated/",
                            "S3DataDistributionType": "ShardedByS3Key",
                        },
                    },
                    "CompressionType": "Gzip",
                    "RecordWrapperType": "RecordIO",
                }],
            }),
        ),
        (
            "output-stated",
            1,
            json!({
                "OutputDataConfig": {
                    "S3OutputPath": "s3://demo/output/",
                    "CompressionType": "NONE",
                    "KmsKeyId": "alias/training-output",
                },
            }),
        ),
        (
            "switches-on",
            1,
            json!({
                "EnableNetworkIsolation": true,
                "EnableInterContainerTrafficEncryption": true,
                "EnableManagedSpotTraining": true,
                "StoppingCondition": { "MaxRuntimeInSeconds": 3600, "MaxWaitTimeInSeconds": 7200 },
            }),
        ),
        (
            "profiling-configured",
            1,
            json!({ "ProfilerConfig": { "S3OutputPath": "s3://demo/profiler/" } }),
        ),
        (
            "profiling-disabled",
            1,
            json!({ "ProfilerConfig": { "DisableProfiler": true } }),
        ),
    ];

    for (case, channels, extra) in rows {
        let emulator = Emulator::new();
        let body = training_body(case, channels, extra);
        emulator.ok("CreateTrainingJob", &body).await;

        let described = emulator
            .ok("DescribeTrainingJob", &json!({ "TrainingJobName": case }))
            .await;

        // Every channel the request carried is echoed, in order, with the three members
        // the service resolves for it.
        let sent = body
            .get("InputDataConfig")
            .and_then(Value::as_array)
            .expect("the body carries channels")
            .clone();
        let reported = described
            .get("InputDataConfig")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: expected InputDataConfig in {described}"))
            .clone();
        assert_eq!(
            reported.len(),
            sent.len(),
            "case {case}: every channel must be echoed, got {described:?}"
        );
        for (index, (sent, reported)) in sent.iter().zip(reported.iter()).enumerate() {
            assert_eq!(
                reported.get("ChannelName"),
                sent.get("ChannelName"),
                "case {case} channel {index}: ChannelName, got {reported:?}"
            );
            assert_eq!(
                reported.get("CompressionType").and_then(Value::as_str),
                Some(
                    sent.get("CompressionType")
                        .and_then(Value::as_str)
                        .unwrap_or("None")
                ),
                "case {case} channel {index}: CompressionType, got {reported:?}"
            );
            assert_eq!(
                reported.get("RecordWrapperType").and_then(Value::as_str),
                sent.get("RecordWrapperType").and_then(Value::as_str),
                "case {case} channel {index}: RecordWrapperType, got {reported:?}"
            );
            assert_eq!(
                reported
                    .pointer("/DataSource/S3DataSource/S3DataDistributionType")
                    .and_then(Value::as_str),
                Some(
                    sent.pointer("/DataSource/S3DataSource/S3DataDistributionType")
                        .and_then(Value::as_str)
                        .unwrap_or("FullyReplicated")
                ),
                "case {case} channel {index}: S3DataDistributionType, got {reported:?}"
            );
            assert_eq!(
                reported.pointer("/DataSource/S3DataSource/S3Uri"),
                sent.pointer("/DataSource/S3DataSource/S3Uri"),
                "case {case} channel {index}: S3Uri, got {reported:?}"
            );
        }

        let sent_output = body.get("OutputDataConfig").expect("an output configuration");
        assert_eq!(
            described
                .pointer("/OutputDataConfig/CompressionType")
                .and_then(Value::as_str),
            Some(
                sent_output
                    .get("CompressionType")
                    .and_then(Value::as_str)
                    .unwrap_or("GZIP")
            ),
            "case {case}: OutputDataConfig.CompressionType, got {described:?}"
        );
        assert_eq!(
            described
                .pointer("/OutputDataConfig/KmsKeyId")
                .and_then(Value::as_str),
            Some(
                sent_output
                    .get("KmsKeyId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
            ),
            "case {case}: OutputDataConfig.KmsKeyId, got {described:?}"
        );
        for member in [
            "EnableNetworkIsolation",
            "EnableInterContainerTrafficEncryption",
            "EnableManagedSpotTraining",
        ] {
            assert_eq!(
                described.get(member).and_then(Value::as_bool),
                Some(body.get(member).and_then(Value::as_bool).unwrap_or(false)),
                "case {case}: {member}, got {described:?}"
            );
        }
        assert_eq!(
            described
                .pointer("/AlgorithmSpecification/EnableSageMakerMetricsTimeSeries")
                .and_then(Value::as_bool),
            Some(false),
            "case {case}: AlgorithmSpecification.EnableSageMakerMetricsTimeSeries, \
             got {described:?}"
        );
        // A job is profiled only where a profiler configuration asked for it, so a job
        // created with none reports no profiling.
        let profiled = body
            .get("ProfilerConfig")
            .is_some_and(|config| {
                !config
                    .get("DisableProfiler")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            });
        assert_eq!(
            described.get("ProfilingStatus").and_then(Value::as_str),
            Some(if profiled { "Enabled" } else { "Disabled" }),
            "case {case}: ProfilingStatus, got {described:?}"
        );
        // The history carries the secondary status the job is in, starting when the job
        // was created.
        let transitions = described
            .get("SecondaryStatusTransitions")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {case}: expected SecondaryStatusTransitions in {described}")
            })
            .clone();
        assert_eq!(
            transitions.first().and_then(|first| first.get("Status")),
            described.get("SecondaryStatus").or(Some(&json!("Starting"))),
            "case {case}: the first transition is the status the job started in, \
             got {described:?}"
        );
        assert_eq!(
            transitions.first().and_then(|first| first.get("StartTime")),
            described.get("CreationTime"),
            "case {case}: the first transition starts when the job was created, \
             got {described:?}"
        );
    }
}

/// RULE: a feature group is deleted only once it EXISTS — while its creation is still
/// owed, `DeleteFeatureGroup` refuses with the 400 `ValidationException` naming the
/// status that stands in the way, and the group survives; once the clock has completed
/// the creation the same call is accepted.
#[tokio::test]
async fn deleting_a_feature_group_that_is_still_being_created_is_refused() {
    // Rows differ along which stores the group keeps, which is independent of the rule:
    // the status is what decides, so every row is refused the same way.
    let rows: Vec<(&str, Value)> = vec![
        ("online-only", json!({ "OnlineStoreConfig": { "EnableOnlineStore": true } })),
        (
            "offline-only",
            json!({
                "OfflineStoreConfig": { "S3StorageConfig": { "S3Uri": "s3://features/off/" } },
                "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/FeatureStoreRole"),
            }),
        ),
    ];

    for (case, extra) in rows {
        let emulator = Emulator::new();
        let mut body = json!({
            "FeatureGroupName": case,
            "RecordIdentifierFeatureName": "customer-id",
            "EventTimeFeatureName": "event-time",
            "FeatureDefinitions": [
                { "FeatureName": "customer-id", "FeatureType": "String" },
                { "FeatureName": "event-time", "FeatureType": "String" },
            ],
        });
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            body[key.as_str()] = value.clone();
        }
        emulator.ok("CreateFeatureGroup", &body).await;

        let (status, refused) = emulator
            .call("DeleteFeatureGroup", &json!({ "FeatureGroupName": case }))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a group still being created cannot be deleted, got {status} \
             with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case}: the refusal's code, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            format!(
                "Validation Error: FeatureGroup with name {case} cannot be deleted, \
                 because FeatureGroupStatus is 'CREATING'."
            ),
            "case {case}: the refusal's message, got {refused:?}"
        );
        // The refused delete left the group exactly where it was.
        let described = emulator
            .ok("DescribeFeatureGroup", &json!({ "FeatureGroupName": case }))
            .await;
        assert_eq!(
            str_field(&described, "FeatureGroupStatus"),
            "Creating",
            "case {case}: the group survives the refusal, got {described:?}"
        );

        // Once the creation the clock owed has fallen due, the same delete is accepted.
        tokio::time::sleep(PAST_CREATED_DUE).await;
        let (status, deleted) = emulator
            .call("DeleteFeatureGroup", &json!({ "FeatureGroupName": case }))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: a created group is deletable, got {status} with {deleted}"
        );
    }
}

/// RULE: the secondary-status history a read reports is the statuses the job has actually
/// been in: one open-ended entry while it is still in its first, and then every stage of the
/// work it went through followed by the terminal one — each starting where the one before it
/// ended, the last carrying what the job reports as its failure — once the clock has settled
/// the job.
#[tokio::test]
async fn the_secondary_status_history_follows_the_statuses_the_job_has_been_in() {
    let emulator = Emulator::new();
    let case = "settling-history";
    emulator
        .ok("CreateTrainingJob", &training_body(case, 1, json!({})))
        .await;

    let running = emulator
        .ok("DescribeTrainingJob", &json!({ "TrainingJobName": case }))
        .await;
    let transitions = history(&running);
    assert_eq!(
        transitions.len(),
        1,
        "case {case}: a job still in its first secondary status reports one entry, \
         got {running:?}"
    );
    assert_eq!(
        transitions[0].get("Status"),
        running.get("SecondaryStatus"),
        "case {case}: the entry names the status the job is in, got {running:?}"
    );
    assert_eq!(
        transitions[0].get("StartTime"),
        running.get("CreationTime"),
        "case {case}: the entry starts when the job was created, got {running:?}"
    );
    assert!(
        transitions[0].get("EndTime").is_none(),
        "case {case}: the status the job is still in has not ended, got {running:?}"
    );

    let settled = poll_until_the_job_settles(&emulator, case).await;
    let transitions = history(&settled);
    assert!(
        transitions.len() >= 2,
        "case {case}: a settled job reports the stages it went through and the status it \
         reached, got {settled:?}"
    );
    for (index, pair) in transitions.windows(2).enumerate() {
        assert_eq!(
            pair[0].get("EndTime"),
            pair[1].get("StartTime"),
            "case {case}: entry {index} must end where the next one begins, got {settled:?}"
        );
    }
    let last = transitions
        .last()
        .unwrap_or_else(|| panic!("case {case}: expected a last entry in {settled:?}"));
    assert_eq!(
        last.get("Status"),
        settled.get("SecondaryStatus"),
        "case {case}: the last entry names the status the job is now in, got {settled:?}"
    );
    assert_eq!(
        last.get("StartTime"),
        settled.get("TrainingEndTime"),
        "case {case}: the terminal entry starts when the job ended, got {settled:?}"
    );
    assert_eq!(
        last.get("StatusMessage"),
        settled.get("FailureReason"),
        "case {case}: the history says what the job's own FailureReason says, \
         got {settled:?}"
    );
}

/// A training job's `SecondaryStatusTransitions`, which every read of a job reports.
fn history(described: &Value) -> Vec<Value> {
    described
        .get("SecondaryStatusTransitions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("expected SecondaryStatusTransitions in {described}")
        })
        .clone()
}

/// Poll `DescribeTrainingJob` for `name` until the job reaches a status nothing leaves on
/// its own, answering that last read.
///
/// The wait is on the CONDITION rather than on a span: the loop looks again until the clock
/// has settled the job, or until a deadline far longer than the span the state gives a job's
/// work has passed.
async fn poll_until_the_job_settles(emulator: &Emulator, name: &str) -> Value {
    /// The `TrainingJobStatus` members nothing leaves on its own.
    const TERMINAL: [&str; 3] = ["Completed", "Failed", "Stopped"];

    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let described = emulator
            .ok("DescribeTrainingJob", &json!({ "TrainingJobName": name }))
            .await;
        if TERMINAL.contains(&str_field(&described, "TrainingJobStatus").as_str()) {
            return described;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the training job {name} must reach a terminal status, last read {described}"
        );
        tokio::time::sleep(SETTLE_POLL_INTERVAL).await;
    }
}
