// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the requests this control plane REFUSES on the strength of a
//! rule the model states in prose rather than in a constraint trait — the classes a
//! boundary validator cannot catch, so the handler is what has to.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, reusing
//! the harness in [`crate::inference_deployment_lifecycle`], so what these tests exercise
//! is what a real SDK client gets.
//!
//! One rule per test, each asserted over a TABLE spanning the class it governs:
//!
//! * a tag key inside the namespace Amazon Web Services keeps for its own tags is written
//!   nowhere — over the key shapes and casings of that namespace, and over two resource
//!   kinds, against the keys next to it that ARE the caller's to write;
//! * a `ResourceConfig` that names no ML compute to launch is refused — over the members
//!   that can be absent or zero, and inside a heterogeneous cluster's instance groups;
//! * an ML storage volume past the account's published quota for it is the modelled limit
//!   failure, at the bound, past it, at the zero the member's doc admits, and with the
//!   account's own utilisation in the sentence;
//! * ML compute past the account's per-instance-type training quota is the modelled limit
//!   failure, counted against the jobs the account is already running;
//! * a HyperPod cluster asking for more instances of a type than the account's quota
//!   allows, or more nodes than one cluster provisions, is the modelled limit failure;
//! * a resource policy that is not an ASCII IAM policy document is refused as a request
//!   failure, leaving the stored document alone;
//! * a listing filtered on a NAMED entity resolves that entity first, so a name no record
//!   answers to is refused rather than answered with an empty page.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The region every request below signs for, so a minted ARN is comparable to an expected
/// one.
const REGION: &str = "us-east-1";

/// The most nodes one HyperPod cluster holds (`state::CLUSTER_NODE_QUOTA`), which is also
/// the most a create may ask its instance groups for.
const CLUSTER_NODE_BOUND: i64 = 1_000;

/// The most instances of one ML instance type an account may run for training jobs
/// (`state::DEFAULT_TRAINING_USAGE_QUOTA`).
const TRAINING_INSTANCE_BOUND: i64 = 1_000;

/// The largest ML storage volume SageMaker provisions for one training job instance, in
/// GiB (`state::TRAINING_VOLUME_SIZE_MAX_GB`: the published maximum size of the Amazon
/// EBS gp2 volume `ResourceConfig$VolumeSizeInGB` names).
const VOLUME_SIZE_MAX_GB: i64 = 16_384;

/// A `CreateTrainingJob` body naming `name` and carrying `resource_config`, with the
/// members the model marks required
/// (`smithy_query(query="operation",operation="CreateTrainingJob")`).
fn training_body(name: &str, resource_config: Value) -> Value {
    json!({
        "TrainingJobName": name,
        "AlgorithmSpecification": {
            "TrainingImage": format!("{ACCOUNT}.dkr.ecr.{REGION}.amazonaws.com/trainer:1"),
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
        "ResourceConfig": resource_config,
        "StoppingCondition": { "MaxRuntimeInSeconds": 3600 },
    })
}

/// One instance group of a HyperPod cluster asking for `count` instances.
fn cluster_group(name: &str, count: i64) -> Value {
    json!({
        "InstanceGroupName": name,
        "InstanceType": "ml.m5.large",
        "InstanceCount": count,
        "ExecutionRole": format!("arn:aws:iam::{ACCOUNT}:role/hyperpod-execution"),
    })
}

/// One instance group asking for `count` instances of an ACCELERATED generation — a family
/// whose published default quota is zero instances, so an account holds no capacity for it
/// until it is granted some.
fn accelerated_cluster_group(name: &str, count: i64) -> Value {
    json!({
        "InstanceGroupName": name,
        "InstanceType": "ml.p5.48xlarge",
        "InstanceCount": count,
        "ExecutionRole": format!("arn:aws:iam::{ACCOUNT}:role/hyperpod-execution"),
    })
}

/// An IAM policy document whose statement id is `sid`, so one document is told from
/// another.
fn policy_with_sid(sid: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"{sid}","Effect":"Allow","Principal":{{"AWS":"*"}},"Action":"sagemaker:DescribeModelPackage","Resource":"*"}}]}}"#
    )
}

/// The tag keys of the resource `arn` names, as `ListTags` reports them.
async fn listed_tag_keys(emulator: &Emulator, arn: &str) -> Vec<String> {
    let listed = emulator.ok("ListTags", &json!({ "ResourceArn": arn })).await;
    listed
        .get("Tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a Tags list in {listed}"))
        .iter()
        .map(|tag| str_field(tag, "Key"))
        .collect()
}

/// RULE: the `aws:` tag-key prefix is Amazon Web Services' own namespace, so a request
/// that asks SageMaker to store a key inside it is refused 400 `ValidationException` and
/// the resource's tag set is left exactly as it was — while a key merely containing or
/// resembling the prefix is the caller's to write.
#[tokio::test]
async fn add_tags_refuses_a_key_in_the_reserved_namespace() {
    // Rows differ along what follows the prefix, the casing the caller wrote it in, and
    // whether the prefix OPENS the key at all.
    let refused: [(&str, &str); 5] = [
        ("the prefix alone", "aws:"),
        ("a key AWS generates", "aws:cloudformation:stack-name"),
        ("upper case", "AWS:created-by"),
        ("mixed case", "Aws:CreatedBy"),
        ("the prefix with a long suffix", "aws:servicecatalog:provisioning-principal-arn"),
    ];
    let accepted: [(&str, &str); 4] = [
        ("an ordinary key", "team"),
        ("the prefix without its colon", "aws"),
        ("the prefix inside the key", "my-aws:thing"),
        ("a key that merely starts with the letters", "awsome"),
    ];

    // Two resource kinds, so the rule is the tag key's and not one resource family's.
    for (kind, operation, body, name_member) in [
        (
            "a model package group",
            "CreateModelPackageGroup",
            json!({ "ModelPackageGroupName": "tagged-group" }),
            "ModelPackageGroupArn",
        ),
        (
            "an experiment",
            "CreateExperiment",
            json!({ "ExperimentName": "tagged-experiment" }),
            "ExperimentArn",
        ),
    ] {
        let emulator = Emulator::new();
        let created = emulator.ok(operation, &body).await;
        let arn = str_field(&created, name_member);

        for (case, key) in refused {
            let (status, answer) = emulator
                .call(
                    "AddTags",
                    &json!({ "ResourceArn": arn, "Tags": [{ "Key": key, "Value": "blocked" }] }),
                )
                .await;
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, "ValidationException"),
                "case {kind} / {case:?}: expected 400 ValidationException, got {status} with \
                 {answer}"
            );
            assert!(
                message(&answer).contains(key),
                "case {kind} / {case:?}: the refusal must name the key it refused, got {}",
                message(&answer)
            );
            let held = listed_tag_keys(&emulator, &arn).await;
            assert!(
                !held.iter().any(|stored| stored == key),
                "case {kind} / {case:?}: a refused key must be stored nowhere, got {held:?}"
            );
        }

        for (case, key) in accepted {
            let added = emulator
                .ok(
                    "AddTags",
                    &json!({ "ResourceArn": arn, "Tags": [{ "Key": key, "Value": "kept" }] }),
                )
                .await;
            let reported: Vec<String> = added
                .get("Tags")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("expected a Tags list in {added}"))
                .iter()
                .map(|tag| str_field(tag, "Key"))
                .collect();
            assert!(
                reported.iter().any(|stored| stored == key),
                "case {kind} / {case:?}: an admitted key must be reported, got {reported:?}"
            );
            let held = listed_tag_keys(&emulator, &arn).await;
            assert!(
                held.iter().any(|stored| stored == key),
                "case {kind} / {case:?}: an admitted key must be stored, got {held:?}"
            );
        }

        // Every refused key is absent from the resource's whole tag set, and only the
        // admitted ones are there.
        let held = listed_tag_keys(&emulator, &arn).await;
        assert_eq!(
            held.len(),
            accepted.len(),
            "case {kind}: only the admitted keys may be stored, got {held:?}"
        );
    }
}

/// RULE: a training job runs on the ML compute its `ResourceConfig` names, so a
/// configuration that names no instance type with a count greater than zero — and no
/// heterogeneous cluster of instance groups that do — is refused 400 `ValidationException`
/// stating the pair it has to carry, and stores no job; a configuration that names
/// launchable compute is admitted and replayed as stored.
#[tokio::test]
async fn create_training_job_refuses_compute_it_cannot_launch() {
    // What a refused configuration is told, from `state::INSTANCE_TYPE_AND_COUNT_REQUIRED`.
    let required = "InstanceType and InstanceCount greater than 0 must be specified";

    // Rows differ along which member is absent, the count itself, whether the compute sits
    // in the configuration's own members or in a heterogeneous cluster's instance groups,
    // and which group of the cluster asks for nothing. `None` is an admitted configuration.
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "one instance and the smallest volume",
            json!({ "InstanceType": "ml.m5.large", "InstanceCount": 1, "VolumeSizeInGB": 1 }),
            true,
        ),
        (
            "a distributed job",
            json!({ "InstanceType": "ml.m5.large", "InstanceCount": 8, "VolumeSizeInGB": 30 }),
            true,
        ),
        (
            "a volume at the published maximum",
            json!({
                "InstanceType": "ml.m5.large",
                "InstanceCount": 1,
                "VolumeSizeInGB": VOLUME_SIZE_MAX_GB,
            }),
            true,
        ),
        (
            "no volume size stated at all",
            json!({ "InstanceType": "ml.m5.large", "InstanceCount": 2 }),
            true,
        ),
        (
            // The member's own doc: SageMaker selects the volume size for an instance type
            // whose storage is fixed, so the modelled minimum of zero is inside the range
            // the service takes.
            "a volume size of zero",
            json!({ "InstanceType": "ml.m5.large", "InstanceCount": 1, "VolumeSizeInGB": 0 }),
            true,
        ),
        (
            "a heterogeneous cluster of two populated groups",
            json!({
                "VolumeSizeInGB": 30,
                "InstanceGroups": [
                    {
                        "InstanceGroupName": "head",
                        "InstanceType": "ml.m5.large",
                        "InstanceCount": 1,
                    },
                    {
                        "InstanceGroupName": "workers",
                        "InstanceType": "ml.m5.xlarge",
                        "InstanceCount": 2,
                    },
                ],
            }),
            true,
        ),
        (
            "an instance count of zero",
            json!({ "InstanceType": "ml.m5.large", "InstanceCount": 0, "VolumeSizeInGB": 30 }),
            false,
        ),
        (
            "no instance count at all",
            json!({ "InstanceType": "ml.m5.large", "VolumeSizeInGB": 30 }),
            false,
        ),
        (
            "a count with no instance type",
            json!({ "InstanceCount": 2, "VolumeSizeInGB": 30 }),
            false,
        ),
        (
            "neither member",
            json!({ "VolumeSizeInGB": 30 }),
            false,
        ),
        (
            "a heterogeneous cluster whose second group asks for nothing",
            json!({
                "VolumeSizeInGB": 30,
                "InstanceGroups": [
                    {
                        "InstanceGroupName": "head",
                        "InstanceType": "ml.m5.large",
                        "InstanceCount": 1,
                    },
                    {
                        "InstanceGroupName": "workers",
                        "InstanceType": "ml.m5.xlarge",
                        "InstanceCount": 0,
                    },
                ],
            }),
            false,
        ),
        (
            "a heterogeneous cluster whose first group asks for nothing",
            json!({
                "VolumeSizeInGB": 30,
                "InstanceGroups": [
                    {
                        "InstanceGroupName": "head",
                        "InstanceType": "ml.m5.large",
                        "InstanceCount": 0,
                    },
                    {
                        "InstanceGroupName": "workers",
                        "InstanceType": "ml.m5.xlarge",
                        "InstanceCount": 2,
                    },
                ],
            }),
            false,
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, config, admitted)) in rows.into_iter().enumerate() {
        let name = format!("compute-case-{index}");
        let (status, answer) = emulator
            .call("CreateTrainingJob", &training_body(&name, config.clone()))
            .await;

        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: launchable compute must be accepted, got {status} with {answer}"
            );
            // Read back through the implemented describe, so the admitted configuration is
            // shown to be the stored one.
            let described = emulator
                .ok("DescribeTrainingJob", &json!({ "TrainingJobName": name }))
                .await;
            assert_eq!(
                described
                    .get("ResourceConfig")
                    .and_then(|held| held.get("InstanceCount")),
                config.get("InstanceCount"),
                "case {case:?}: the admitted InstanceCount must be replayed, got {described}"
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
            required,
            "case {case:?}: the refusal's message, got {answer}"
        );
        let (status, _) = emulator
            .call("DescribeTrainingJob", &json!({ "TrainingJobName": name }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create must store no training job"
        );
    }
}

/// RULE: the ML storage volume SageMaker provisions for a training job instance is bounded
/// by the account's published quota for it, so a `VolumeSizeInGB` past that bound is the
/// modelled `ResourceLimitExceeded` stating the quota entry, its limit, the account's
/// utilisation of it and this request's delta; a size at the bound — and the zero the
/// member's own doc admits — is accepted.
#[tokio::test]
async fn create_training_job_refuses_a_volume_past_the_published_quota() {
    let config = |size: i64| {
        json!({ "InstanceType": "ml.m5.large", "InstanceCount": 1, "VolumeSizeInGB": size })
    };

    // Rows differ along where the size sits relative to the published bound.
    for (case, size, admitted) in [
        ("the size SageMaker selects for itself", 0, true),
        ("at the published maximum", VOLUME_SIZE_MAX_GB, true),
        ("one past the published maximum", VOLUME_SIZE_MAX_GB + 1, false),
        ("an absurd size", 999_999_999, false),
    ] {
        let emulator = Emulator::new();
        let (status, answer) = emulator
            .call("CreateTrainingJob", &training_body("volume-job", config(size)))
            .await;
        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: a volume inside the quota must be accepted, got {status} with \
                 {answer}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ResourceLimitExceeded"),
            "case {case:?}: expected 400 ResourceLimitExceeded, got {status} with {answer}"
        );
        assert_eq!(
            message(&answer),
            format!(
                "The account-level service limit 'Size of EBS volume for a training job \
                 instance' is {VOLUME_SIZE_MAX_GB} GB, with current utilization of 0 GB and a \
                 request delta of {size} GB. Please use AWS Service Quotas to request an \
                 increase for this quota. If AWS Service Quotas is not available, contact AWS \
                 support to request an increase for this quota."
            ),
            "case {case:?}: the refusal's message, got {answer}"
        );
        let (status, _) = emulator
            .call(
                "DescribeTrainingJob",
                &json!({ "TrainingJobName": "volume-job" }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create must store no training job"
        );
    }

    // The utilisation the refusal states is the account's own: a job already running holds
    // its storage, so the same refusal reports that volume rather than zero.
    let emulator = Emulator::new();
    let held = 512;
    let (status, answer) = emulator
        .call(
            "CreateTrainingJob",
            &training_body("volume-held", config(held)),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"a job holding a volume\": expected HTTP 200, got {status} with {answer}"
    );
    let asked = VOLUME_SIZE_MAX_GB + 1;
    let (status, answer) = emulator
        .call(
            "CreateTrainingJob",
            &training_body("volume-next", config(asked)),
        )
        .await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (400, "ResourceLimitExceeded"),
        "case \"a volume past the quota beside a held one\": expected 400 \
         ResourceLimitExceeded, got {status} with {answer}"
    );
    assert!(
        message(&answer).contains(&format!("current utilization of {held} GB")),
        "case \"a volume past the quota beside a held one\": the refusal must state the \
         account's own utilisation, got {}",
        message(&answer)
    );
}

/// RULE: the instances a training job asks for are weighed against the account's quota for
/// that INSTANCE TYPE, counted over the jobs it is already running — so a request past the
/// quota is the modelled `ResourceLimitExceeded` naming that quota entry, one at the quota
/// is admitted, and what one job holds is what the next request is measured against.
#[tokio::test]
async fn create_training_job_refuses_more_instances_than_the_account_may_run() {
    let config = |instance_type: &str, count: i64| {
        json!({
            "InstanceType": instance_type,
            "InstanceCount": count,
            "VolumeSizeInGB": 30,
        })
    };

    // A request AT the quota is admitted, and one past it is refused.
    for (case, count, admitted) in [
        ("at the quota", TRAINING_INSTANCE_BOUND, true),
        ("one past the quota", TRAINING_INSTANCE_BOUND + 1, false),
        ("far past the quota", TRAINING_INSTANCE_BOUND * 1_000, false),
    ] {
        let emulator = Emulator::new();
        let (status, answer) = emulator
            .call(
                "CreateTrainingJob",
                &training_body("quota-job", config("ml.m5.large", count)),
            )
            .await;
        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: a request at the quota must be accepted, got {status} with \
                 {answer}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ResourceLimitExceeded"),
            "case {case:?}: expected 400 ResourceLimitExceeded, got {status} with {answer}"
        );
        assert!(
            message(&answer).contains("ml.m5.large for training job usage"),
            "case {case:?}: the refusal must name the quota entry, got {}",
            message(&answer)
        );
    }

    // The utilisation is the account's own: a job already running holds its instances, so
    // the same request that was admitted first is refused second — and a request for
    // another instance type is measured against that type's own quota.
    let emulator = Emulator::new();
    let half = TRAINING_INSTANCE_BOUND / 2 + 1;
    let first = emulator
        .call(
            "CreateTrainingJob",
            &training_body("usage-one", config("ml.m5.large", half)),
        )
        .await;
    assert_eq!(
        first.0, 200,
        "case \"the first half of the quota\": expected HTTP 200, got {} with {}",
        first.0, first.1
    );
    let (status, answer) = emulator
        .call(
            "CreateTrainingJob",
            &training_body("usage-two", config("ml.m5.large", half)),
        )
        .await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (400, "ResourceLimitExceeded"),
        "case \"the second half of the same quota\": expected 400 ResourceLimitExceeded, got \
         {status} with {answer}"
    );
    let (status, answer) = emulator
        .call(
            "CreateTrainingJob",
            &training_body("usage-three", config("ml.m5.xlarge", half)),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"another instance type's quota\": expected HTTP 200, got {status} with {answer}"
    );
}

/// RULE: a created cluster's groups are brought up to the instance counts they declare,
/// so the instances they ask for are weighed against the account's published quota for
/// their INSTANCE TYPE — zero for the accelerated generations an account must be granted
/// capacity for — and against the bound on the nodes one cluster holds; a create past
/// either is the modelled `ResourceLimitExceeded` and mints no cluster, while one inside
/// both is admitted and describable.
#[tokio::test]
async fn create_cluster_refuses_more_nodes_than_one_cluster_provisions() {
    // Rows differ along how the nodes are spread over groups, where the total sits
    // relative to the bound, and whether the type is one the account has capacity for.
    let rows: Vec<(&str, Vec<Value>, Option<&str>)> = vec![
        ("one node", vec![cluster_group("workers", 1)], None),
        (
            "a group asking for no instances",
            vec![cluster_group("idle", 0)],
            None,
        ),
        (
            "the whole bound in one group",
            vec![cluster_group("workers", CLUSTER_NODE_BOUND)],
            None,
        ),
        (
            "the bound split over two groups",
            vec![
                cluster_group("head", 1),
                cluster_group("workers", CLUSTER_NODE_BOUND - 1),
            ],
            None,
        ),
        (
            "one node past the bound",
            vec![cluster_group("workers", CLUSTER_NODE_BOUND + 1)],
            Some("ResourceLimitExceeded"),
        ),
        (
            "two groups summing one past the bound",
            vec![
                cluster_group("head", 2),
                cluster_group("workers", CLUSTER_NODE_BOUND - 1),
            ],
            Some("ResourceLimitExceeded"),
        ),
        (
            "the published maximum of the modelled range",
            vec![cluster_group("workers", 6_758)],
            Some("ResourceLimitExceeded"),
        ),
        (
            "one past the modelled maximum",
            vec![cluster_group("workers", 6_759)],
            Some("ValidationException"),
        ),
        (
            "an absurd count",
            vec![cluster_group("workers", 999_999_999)],
            Some("ValidationException"),
        ),
        (
            "one instance of an accelerated generation the account has no capacity for",
            vec![accelerated_cluster_group("gpu", 1)],
            Some("ResourceLimitExceeded"),
        ),
        (
            "a whole group of an accelerated generation",
            vec![accelerated_cluster_group("gpu", 8)],
            Some("ResourceLimitExceeded"),
        ),
        (
            "an admitted group beside one of an accelerated generation",
            vec![
                cluster_group("workers", 2),
                accelerated_cluster_group("gpu", 2),
            ],
            Some("ResourceLimitExceeded"),
        ),
    ];

    for (index, (case, groups, refused_with)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("nodes-case-{index}");
        let (status, answer) = emulator
            .call(
                "CreateCluster",
                &json!({ "ClusterName": name, "InstanceGroups": groups }),
            )
            .await;

        let Some(code) = refused_with else {
            assert_eq!(
                status, 200,
                "case {case:?}: a cluster inside the bound must be accepted, got {status} \
                 with {answer}"
            );
            let described = emulator
                .ok("DescribeCluster", &json!({ "ClusterName": name }))
                .await;
            assert_eq!(
                str_field(&described, "ClusterArn"),
                str_field(&answer, "ClusterArn"),
                "case {case:?}: the created cluster must be describable, got {described}"
            );
            continue;
        };

        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, code),
            "case {case:?}: expected 400 {code}, got {status} with {answer}"
        );
        if code == "ResourceLimitExceeded" {
            assert!(
                message(&answer).contains("service limit"),
                "case {case:?}: a limit refusal states the quota it is past, got {}",
                message(&answer)
            );
        }
        let (status, _) = emulator
            .call("DescribeCluster", &json!({ "ClusterName": name }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create must mint no cluster"
        );
    }

    // The limit refusal states the quota ENTRY, its limit, the account's utilisation of it
    // and this request's delta — the four numbers a client reads to decide between asking
    // for fewer instances and requesting an increase.
    let emulator = Emulator::new();
    let requested = 8;
    let (status, answer) = emulator
        .call(
            "CreateCluster",
            &json!({
                "ClusterName": "accelerated-cluster",
                "InstanceGroups": [accelerated_cluster_group("gpu", requested)],
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (400, "ResourceLimitExceeded"),
        "case \"an accelerated generation\": expected 400 ResourceLimitExceeded, got {status} \
         with {answer}"
    );
    assert_eq!(
        message(&answer),
        format!(
            "The account-level service limit 'ml.p5.48xlarge for cluster usage' is 0 Instances, \
             with current utilization of 0 Instances and a request delta of {requested} \
             Instances. Please use AWS Service Quotas to request an increase for this quota. If \
             AWS Service Quotas is not available, contact AWS support to request an increase for \
             this quota."
        ),
        "case \"an accelerated generation\": the refusal's message, got {answer}"
    );

    // The utilisation is the account's own: the clusters it already holds take capacity, so
    // the same request that was admitted first is refused second.
    let emulator = Emulator::new();
    let half = CLUSTER_NODE_BOUND / 2 + 1;
    let (status, answer) = emulator
        .call(
            "CreateCluster",
            &json!({
                "ClusterName": "usage-one",
                "InstanceGroups": [cluster_group("workers", half)],
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the first half of the quota\": expected HTTP 200, got {status} with {answer}"
    );
    let (status, answer) = emulator
        .call(
            "CreateCluster",
            &json!({
                "ClusterName": "usage-two",
                "InstanceGroups": [cluster_group("workers", half)],
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (400, "ResourceLimitExceeded"),
        "case \"the second half of the same quota\": expected 400 ResourceLimitExceeded, got \
         {status} with {answer}"
    );
}

/// RULE: a resource policy is an ASCII IAM policy document, so a document that is not one
/// is refused 400 `ValidationException` — the class for a request the caller has to change,
/// not this operation's modelled `ConflictException` — and the group keeps the document it
/// carried.
#[tokio::test]
async fn put_model_package_group_policy_refuses_a_document_that_is_not_an_ascii_policy() {
    // Rows differ along WHY the document is not a policy: non-ASCII content (in three
    // different elements), JSON that does not parse, JSON that is not an object, and
    // objects that parse but state no policy.
    let refused: Vec<(&str, String)> = vec![
        ("a non-ASCII statement id", policy_with_sid("Modèle")),
        (
            "a non-ASCII condition value",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"sagemaker:DescribeModelPackage","Condition":{"StringEquals":{"aws:ResourceTag/team":"données"}}}]}"#
                .to_string(),
        ),
        (
            "a non-ASCII action",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"sagemaker:描述"}]}"#
                .to_string(),
        ),
        ("text that is not JSON", "not a policy at all".to_string()),
        (
            "JSON that does not close",
            r#"{"Version":"2012-10-17","Statement":["#.to_string(),
        ),
        ("a JSON array", r#"[{"Effect":"Allow"}]"#.to_string()),
        ("an empty object", "{}".to_string()),
        (
            "a statement that is not an object",
            r#"{"Version":"2012-10-17","Statement":["allow it"]}"#.to_string(),
        ),
        (
            "an element the document level does not define",
            r#"{"Version":"2012-10-17","NotAValidPolicyKey":1,"Statement":[]}"#.to_string(),
        ),
        (
            "an element a statement does not define",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"sagemaker:*","Grant":"*"}]}"#
                .to_string(),
        ),
    ];

    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateModelPackageGroup",
            &json!({ "ModelPackageGroupName": "policy-group" }),
        )
        .await;
    // The document the group really carries, so every refusal below is shown to change
    // nothing.
    let stored = policy_with_sid("Stored");
    emulator
        .ok(
            "PutModelPackageGroupPolicy",
            &json!({ "ModelPackageGroupName": "policy-group", "ResourcePolicy": stored }),
        )
        .await;

    for (case, document) in refused {
        let (status, answer) = emulator
            .call(
                "PutModelPackageGroupPolicy",
                &json!({ "ModelPackageGroupName": "policy-group", "ResourcePolicy": document }),
            )
            .await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {answer}"
        );
        let read = emulator
            .ok(
                "GetModelPackageGroupPolicy",
                &json!({ "ModelPackageGroupName": "policy-group" }),
            )
            .await;
        assert_eq!(
            str_field(&read, "ResourcePolicy"),
            stored,
            "case {case:?}: a refused put must leave the stored document alone, got {read}"
        );
    }

    // An ASCII policy of the same shape as the refused non-ASCII one IS taken, so the rule
    // is about the document's characters and not about its shape.
    let ascii = policy_with_sid("Modele");
    emulator
        .ok(
            "PutModelPackageGroupPolicy",
            &json!({ "ModelPackageGroupName": "policy-group", "ResourcePolicy": ascii }),
        )
        .await;
    let read = emulator
        .ok(
            "GetModelPackageGroupPolicy",
            &json!({ "ModelPackageGroupName": "policy-group" }),
        )
        .await;
    assert_eq!(
        str_field(&read, "ResourcePolicy"),
        ascii,
        "case \"an ASCII policy\": the accepted document must read back, got {read}"
    );
}

/// RULE: a listing filtered on an entity this service NAMES resolves that entity first, so
/// a `TrialName` or `ExperimentName` no record answers to is refused with the modelled
/// `ResourceNotFound` carrying that entity's own not-found sentence — while a stored entity
/// holding no component, and a `SourceArn` (which names no record of this service), answer
/// an empty page.
#[tokio::test]
async fn list_trial_components_refuses_a_filter_naming_no_entity() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateExperiment", &json!({ "ExperimentName": "exp-held" }))
        .await;
    emulator
        .ok(
            "CreateTrial",
            &json!({ "TrialName": "trial-held", "ExperimentName": "exp-held" }),
        )
        .await;
    emulator
        .ok(
            "CreateTrialComponent",
            &json!({ "TrialComponentName": "component-held" }),
        )
        .await;
    emulator
        .ok(
            "AssociateTrialComponent",
            &json!({ "TrialName": "trial-held", "TrialComponentName": "component-held" }),
        )
        .await;

    // Rows differ along which filter is sent, whether the entity it names is stored, and
    // the shape of the name — including a casing the stores fold to the stored record.
    let served: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "a stored trial holding one component",
            json!({ "TrialName": "trial-held" }),
            vec!["component-held"],
        ),
        (
            "a stored trial named in another casing",
            json!({ "TrialName": "TRIAL-HELD" }),
            vec!["component-held"],
        ),
        (
            "a stored experiment",
            json!({ "ExperimentName": "exp-held" }),
            vec!["component-held"],
        ),
        (
            "no entity filter at all",
            json!({}),
            vec!["component-held"],
        ),
        (
            "a source ARN naming no component's source",
            json!({
                "SourceArn": format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:training-job/absent"),
            }),
            vec![],
        ),
    ];
    for (case, filter, expected) in served {
        let listed = emulator.ok("ListTrialComponents", &filter).await;
        let names: Vec<String> = listed
            .get("TrialComponentSummaries")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("expected a TrialComponentSummaries list in {listed}"))
            .iter()
            .map(|summary| str_field(summary, "TrialComponentName"))
            .collect();
        assert_eq!(
            names,
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: the listing's selection, got {listed}"
        );
    }

    // A stored entity that holds nothing is an empty page, not a refusal.
    emulator
        .ok(
            "CreateTrial",
            &json!({ "TrialName": "trial-empty", "ExperimentName": "exp-held" }),
        )
        .await;
    let listed = emulator
        .ok("ListTrialComponents", &json!({ "TrialName": "trial-empty" }))
        .await;
    assert_eq!(
        listed
            .get("TrialComponentSummaries")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "case \"a stored trial holding nothing\": expected an empty page, got {listed}"
    );

    // Rows differ along which filter names the absent entity and the shape of the name.
    let refused: Vec<(&str, Value, String)> = vec![
        (
            "a trial that was never created",
            json!({ "TrialName": "trial-absent" }),
            format!(
                "Trial 'arn:aws:sagemaker:{REGION}:{ACCOUNT}:experiment-trial/trial-absent' does \
                 not exist."
            ),
        ),
        (
            "a trial named like a stored experiment",
            json!({ "TrialName": "exp-held" }),
            format!(
                "Trial 'arn:aws:sagemaker:{REGION}:{ACCOUNT}:experiment-trial/exp-held' does not \
                 exist."
            ),
        ),
        (
            "an experiment that was never created",
            json!({ "ExperimentName": "exp-absent" }),
            format!(
                "Experiment 'arn:aws:sagemaker:{REGION}:{ACCOUNT}:experiment/exp-absent' does \
                 not exist."
            ),
        ),
        (
            "an experiment named like a stored trial",
            json!({ "ExperimentName": "trial-held" }),
            format!(
                "Experiment 'arn:aws:sagemaker:{REGION}:{ACCOUNT}:experiment/trial-held' does \
                 not exist."
            ),
        ),
    ];
    for (case, filter, expected) in refused {
        let (status, answer) = emulator.call("ListTrialComponents", &filter).await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ResourceNotFound"),
            "case {case:?}: expected 400 ResourceNotFound, got {status} with {answer}"
        );
        assert_eq!(
            message(&answer),
            expected,
            "case {case:?}: the refusal's message, got {answer}"
        );
    }
}
