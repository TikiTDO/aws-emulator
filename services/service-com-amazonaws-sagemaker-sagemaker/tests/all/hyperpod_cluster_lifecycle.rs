// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker HyperPod's cluster control plane:
//! `CreateCluster` and the reader it is proved through, `DescribeCluster`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call:
//!
//! * a `ClusterArn` is `…:cluster/[a-z0-9]{12}` — a MINTED id, not the name — for every
//!   region and name shape, and every minted id is distinct;
//! * `DescribeCluster` replays what `CreateCluster` stored, for every optional member of
//!   the request, reached by name or by ARN;
//! * a cluster and its instance groups are `Creating` with nothing realised until the
//!   clock owes `InService`, after which the realised (`Current*`) half of each details
//!   shape equals the declared (`Desired*`) half;
//! * a name already in use is refused with `ResourceInUse`, an unknown identifier with
//!   `ResourceNotFound`, and the rule the model cannot express — `Orchestrator` naming
//!   exactly one configuration — with `ValidationException`.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Long enough to pass `state::CLUSTER_IN_SERVICE_AFTER` (300 ms).
const PAST_IN_SERVICE_DUE: Duration = Duration::from_millis(600);

/// An IAM role ARN matching `RoleArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`).
const EXECUTION_ROLE: &str = "arn:aws:iam::000000000000:role/HyperPodClusterRole";

/// An EKS cluster ARN matching `EksClusterArn`'s modelled pattern.
const EKS_CLUSTER_ARN: &str = "arn:aws:eks:us-east-1:000000000000:cluster/orchestrator";

/// One emulator: the shared engine plus the SageMaker service mounted on the router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not SigV4; the request below carries a
        // credential scope only so the ARN region is the one the caller signed for.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `target` with a SigV4 credential scope naming `region`.
    async fn call_in(&self, region: &str, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/sagemaker/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.dispatch(request).await
    }

    /// Call `target` unsigned: no credential scope, so the ARN region falls back to the
    /// emulator default.
    async fn call_unsigned(&self, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.dispatch(request).await
    }

    async fn dispatch(&self, request: http::Request<bytes::Bytes>) -> (u16, Value) {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    async fn call(&self, target: &str, body: &Value) -> (u16, Value) {
        self.call_in("us-east-1", target, body).await
    }

    /// Call `target` and require a 200, returning the parsed output.
    async fn ok(&self, target: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(target, body).await;
        assert_eq!(
            status, 200,
            "case {target} {body}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// `CreateCluster` with `body`, answering the minted `ClusterArn`.
    async fn create_cluster(&self, body: &Value) -> String {
        let created = self.ok("CreateCluster", body).await;
        created
            .get("ClusterArn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("expected a ClusterArn in {created}"))
            .to_string()
    }

    /// `DescribeCluster` for `name_or_arn`.
    async fn describe_cluster(&self, name_or_arn: &str) -> Value {
        self.ok("DescribeCluster", &json!({ "ClusterName": name_or_arn }))
            .await
    }
}

/// The `__type` an error body carries, with any `namespace#` prefix stripped.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The value at a dotted path (`"InstanceGroups.0.Status"`), or `None` when any segment
/// is absent.
fn at<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cursor = value;
    for segment in path.split('.') {
        cursor = match segment.parse::<usize>() {
            Ok(index) => cursor.get(index)?,
            Err(_) => cursor.get(segment)?,
        };
    }
    Some(cursor)
}

/// `body` with `ClusterName` set to `name`.
fn named(name: &str, body: &Value) -> Value {
    let mut object = body.as_object().cloned().unwrap_or_else(Map::new);
    object.insert("ClusterName".to_string(), json!(name));
    Value::Object(object)
}

/// One instance group: the members `ClusterInstanceGroupSpecification` marks required,
/// plus whatever `extra` adds.
fn instance_group(name: &str, count: i64, instance_type: &str, extra: Value) -> Value {
    let mut group = json!({
        "InstanceGroupName": name,
        "InstanceCount": count,
        "InstanceType": instance_type,
        "ExecutionRole": EXECUTION_ROLE,
    });
    let object = group.as_object_mut().expect("a JSON object");
    for (member, value) in extra.as_object().cloned().unwrap_or_else(Map::new) {
        object.insert(member, value);
    }
    group
}

/// The simplest acceptable cluster body: one one-instance group.
fn one_group_body() -> Value {
    json!({ "InstanceGroups": [instance_group("workers", 1, "ml.m5.large", json!({}))] })
}

/// The `[a-z0-9]{12}` id segment of a `ClusterArn`.
fn cluster_id(arn: &str) -> &str {
    arn.rsplit("cluster/")
        .next()
        .unwrap_or_else(|| panic!("expected a cluster ARN, got {arn:?}"))
}

/// Whether `candidate` is the `[a-z0-9]{12}` segment `ClusterArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:cluster/[a-z0-9]{12}$`) requires.
fn is_cluster_id(candidate: &str) -> bool {
    candidate.len() == 12
        && candidate
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
}

#[tokio::test]
async fn a_cluster_arn_names_the_signed_region_and_a_minted_twelve_character_id() {
    // The name shapes `ClusterName`'s pattern (`^[a-zA-Z0-9](-*[a-zA-Z0-9])*$`, len
    // 1..63) spans: both length bounds, digits only, runs of hyphens, mixed case.
    let longest = format!("c{}", "x".repeat(62));
    let names = [
        ("shortest allowed", "a".to_string()),
        ("longest allowed", longest.clone()),
        ("digits only", "90210".to_string()),
        ("hyphen runs", "a-b--c".to_string()),
        ("mixed case", "MixedCaseCluster".to_string()),
    ];
    // Regions a caller can sign for, plus the unsigned fallback asserted below.
    let regions = ["us-east-1", "eu-west-2", "ap-southeast-4"];

    let mut ids = BTreeSet::new();
    for (shape, cluster) in &names {
        for region in regions {
            // One emulator per region, so the same name can be created in each.
            let emulator = Emulator::new();
            let (status, created) = emulator
                .call_in(region, "CreateCluster", &named(cluster, &one_group_body()))
                .await;
            assert_eq!(
                status, 200,
                "case {shape:?} in {region}: expected HTTP 200, got {status} with {created}"
            );
            let arn = created
                .get("ClusterArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {shape:?} in {region}: no ClusterArn in {created}"))
                .to_string();
            let id = cluster_id(&arn);
            assert_eq!(
                arn,
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:cluster/{id}"),
                "case {shape:?} in {region}: expected an ARN in the signed region, got {arn:?}"
            );
            assert!(
                is_cluster_id(id),
                "case {shape:?} in {region}: expected a 12-character [a-z0-9] id, got {id:?}"
            );
            assert_ne!(
                id, cluster,
                "case {shape:?} in {region}: the ARN carries a minted id, never the name, \
                 got {arn:?}"
            );
            assert!(
                ids.insert(id.to_string()),
                "case {shape:?} in {region}: id {id:?} was minted twice"
            );
        }
    }

    // Unsigned: no credential scope, so the ARN carries the emulator's default region.
    let emulator = Emulator::new();
    let (status, created) = emulator
        .call_unsigned("CreateCluster", &named("unsigned", &one_group_body()))
        .await;
    assert_eq!(
        status, 200,
        "case unsigned: expected HTTP 200, got {status} with {created}"
    );
    let arn = created["ClusterArn"].as_str().unwrap_or_default();
    assert_eq!(
        arn,
        format!(
            "arn:aws:sagemaker:us-east-1:{ACCOUNT}:cluster/{}",
            cluster_id(arn)
        ),
        "case unsigned: expected the default-region ARN, got {arn:?}"
    );
}

#[tokio::test]
async fn every_minted_cluster_id_is_distinct() {
    let emulator = Emulator::new();
    let mut ids = BTreeSet::new();
    for index in 0..25 {
        let arn = emulator
            .create_cluster(&named(&format!("fleet-{index}"), &one_group_body()))
            .await;
        let id = cluster_id(&arn).to_string();
        assert!(
            is_cluster_id(&id),
            "create {index}: expected a 12-character [a-z0-9] id, got {id:?}"
        );
        assert!(ids.insert(id.clone()), "create {index}: id {id:?} repeated");
    }
}

/// RULE: `DescribeCluster` reports the mode the cluster provisions its nodes in for every
/// cluster — the one the create named, and otherwise the service's own mode, which is the
/// one member the modelled `ClusterNodeProvisioningMode` enum carries.
#[tokio::test]
async fn a_cluster_reports_the_node_provisioning_mode_it_runs_in() {
    // The modelled value set, read from the model rather than transcribed
    // (`smithy_query(query="shape",shape="ClusterNodeProvisioningMode")`, mirrored by the
    // generated `types::CLUSTER_NODE_PROVISIONING_MODE_VALUES`).
    const MODES: &[&str] = &["Continuous"];
    let default_mode = MODES.first().copied().expect("one modelled mode");

    // Rows differ along what the rule generalises over: whether the create names the mode
    // at all, the orchestrator (none, EKS, Slurm), the number of instance groups and the
    // Region signed for.
    let rows: Vec<(&str, &str, Value, Option<&str>)> = vec![
        ("no mode, no orchestrator", "us-east-1", one_group_body(), None),
        (
            "no mode, an EKS orchestrator",
            "eu-west-2",
            json!({
                "InstanceGroups": [instance_group("workers", 1, "ml.m5.large", json!({}))],
                "Orchestrator": { "Eks": { "ClusterArn": EKS_CLUSTER_ARN } },
            }),
            None,
        ),
        (
            "no mode, a Slurm orchestrator and two groups",
            "ap-southeast-1",
            json!({
                "InstanceGroups": [
                    instance_group("controller", 1, "ml.m5.large", json!({})),
                    instance_group("compute", 2, "ml.c5.large", json!({})),
                ],
                "Orchestrator": { "Slurm": { "SlurmConfigStrategy": "Managed" } },
            }),
            None,
        ),
        (
            "the mode named explicitly",
            "us-east-1",
            json!({
                "InstanceGroups": [instance_group("workers", 1, "ml.m5.large", json!({}))],
                "Orchestrator": { "Eks": { "ClusterArn": EKS_CLUSTER_ARN } },
                "NodeProvisioningMode": "Continuous",
            }),
            Some("Continuous"),
        ),
    ];

    for (index, (case, region, body, named_mode)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("provisioning-{index}");
        let (status, created) = emulator
            .call_in(region, "CreateCluster", &named(&name, body))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateCluster, got {status} with {created}"
        );

        // Reported while the cluster is still provisioning and once it is `InService`:
        // the mode is a property of the cluster, not of its status.
        for stage in ["while provisioning", "once in service"] {
            let (status, described) = emulator
                .call_in(region, "DescribeCluster", &json!({ "ClusterName": &name }))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} {stage}: DescribeCluster, got {status} with {described}"
            );
            assert_eq!(
                described.get("NodeProvisioningMode").and_then(Value::as_str),
                Some(named_mode.unwrap_or(default_mode)),
                "case {case:?} {stage}: NodeProvisioningMode, got {described:?}"
            );
            if stage == "while provisioning" {
                tokio::time::sleep(PAST_IN_SERVICE_DUE).await;
            }
        }
    }
}

/// One row of the create/describe round trip: a request, the members the reader must
/// report, and the members it must leave out.
struct RoundTrip {
    case: &'static str,
    request: Value,
    expected: Vec<(&'static str, Value)>,
    absent: Vec<&'static str>,
}

#[tokio::test]
async fn describe_cluster_replays_what_create_cluster_stored() {
    // The rows differ along the request's shape: how many instance groups, which
    // optional members each carries, whether the groups are restricted, and which
    // orchestrator and cluster-wide options are set. Enum values below are the modelled
    // ones (`ClusterInstanceType`, `ClusterSlurmNodeType`, `ClusterInterfaceType`,
    // `ClusterNodeRecovery`, `ClusterConfigMode`, `ClusterFSxLustreDeletionPolicy`,
    // `DeepHealthCheckType`, `ClusterKubernetesTaintEffect`).
    let cases = [
        RoundTrip {
            case: "a single minimal instance group",
            request: one_group_body(),
            expected: vec![
                ("ClusterStatus", json!("InService")),
                // The create named no `NodeProvisioningMode`, so the cluster runs in — and
                // reports — the service's own mode (the one member the modelled
                // `ClusterNodeProvisioningMode` carries).
                ("NodeProvisioningMode", json!("Continuous")),
                ("InstanceGroups.0.InstanceGroupName", json!("workers")),
                ("InstanceGroups.0.InstanceType", json!("ml.m5.large")),
                ("InstanceGroups.0.ExecutionRole", json!(EXECUTION_ROLE)),
                ("InstanceGroups.0.CurrentCount", json!(1)),
                ("InstanceGroups.0.TargetCount", json!(1)),
                ("InstanceGroups.0.Status", json!("InService")),
            ],
            absent: vec![
                "FailureMessage",
                "VpcConfig",
                "Orchestrator",
                "AutoScaling",
                "ClusterRole",
                "NodeRecovery",
                "TieredStorageConfig",
                "RestrictedInstanceGroups",
                "RestrictedInstanceGroupsConfig",
                "InstanceGroups.1",
            ],
            },
        RoundTrip {
            case: "two groups, in request order, with counts, threads, storage and checks",
            request: json!({
                "InstanceGroups": [
                    instance_group("alpha", 2, "ml.c5.large", json!({
                        "MinInstanceCount": 1,
                        "ThreadsPerCore": 2,
                        "InstanceStorageConfigs": [
                            { "EbsVolumeConfig": { "VolumeSizeInGB": 500 } },
                        ],
                        "OnStartDeepHealthChecks": ["InstanceStress", "InstanceConnectivity"],
                    })),
                    instance_group("beta", 0, "ml.t3.medium", json!({})),
                ],
            }),
            expected: vec![
                ("InstanceGroups.0.InstanceGroupName", json!("alpha")),
                ("InstanceGroups.0.CurrentCount", json!(2)),
                ("InstanceGroups.0.MinCount", json!(1)),
                ("InstanceGroups.0.ThreadsPerCore", json!(2)),
                (
                    "InstanceGroups.0.InstanceStorageConfigs.0.EbsVolumeConfig.VolumeSizeInGB",
                    json!(500),
                ),
                (
                    "InstanceGroups.0.OnStartDeepHealthChecks.1",
                    json!("InstanceConnectivity"),
                ),
                ("InstanceGroups.1.InstanceGroupName", json!("beta")),
                ("InstanceGroups.1.CurrentCount", json!(0)),
                ("InstanceGroups.1.TargetCount", json!(0)),
            ],
            absent: vec!["InstanceGroups.2", "InstanceGroups.1.MinCount"],
        },
        RoundTrip {
            case: "an image, a release version and a Kubernetes configuration reconcile",
            request: json!({
                "InstanceGroups": [instance_group("k8s", 1, "ml.g5.xlarge", json!({
                    "ImageId": "default",
                    "ImageReleaseVersion": "1.2.3",
                    "KubernetesConfig": {
                        "Labels": { "team": "research" },
                        "Taints": [{ "Key": "gpu", "Value": "true", "Effect": "NoSchedule" }],
                    },
                }))],
            }),
            expected: vec![
                ("InstanceGroups.0.CurrentImageId", json!("default")),
                ("InstanceGroups.0.DesiredImageId", json!("default")),
                ("InstanceGroups.0.CurrentImageReleaseVersion", json!("1.2.3")),
                ("InstanceGroups.0.DesiredImageReleaseVersion", json!("1.2.3")),
                (
                    "InstanceGroups.0.KubernetesConfig.CurrentLabels.team",
                    json!("research"),
                ),
                (
                    "InstanceGroups.0.KubernetesConfig.DesiredLabels.team",
                    json!("research"),
                ),
                (
                    "InstanceGroups.0.KubernetesConfig.CurrentTaints.0.Effect",
                    json!("NoSchedule"),
                ),
                (
                    "InstanceGroups.0.KubernetesConfig.DesiredTaints.0.Key",
                    json!("gpu"),
                ),
            ],
            absent: vec!["InstanceGroups.0.ImageVersionStatus"],
        },
        RoundTrip {
            case: "a lifecycle config, a Slurm node type, a network interface and \
                   instance requirements",
            request: json!({
                "InstanceGroups": [
                    instance_group("controller", 1, "ml.m5.xlarge", json!({
                        "LifeCycleConfig": {
                            "SourceS3Uri": "s3://sagemaker-lifecycle/scripts/",
                            "OnCreate": "on_create.sh",
                        },
                        "SlurmConfig": { "NodeType": "Controller" },
                        "NetworkInterface": { "InterfaceType": "efa" },
                    })),
                    json!({
                        "InstanceGroupName": "flexible",
                        "InstanceCount": 3,
                        "ExecutionRole": EXECUTION_ROLE,
                        "InstanceRequirements": {
                            "InstanceTypes": ["ml.m5.large", "ml.m5.xlarge"],
                        },
                    }),
                ],
            }),
            expected: vec![
                (
                    "InstanceGroups.0.LifeCycleConfig.OnCreate",
                    json!("on_create.sh"),
                ),
                ("InstanceGroups.0.SlurmConfig.NodeType", json!("Controller")),
                (
                    "InstanceGroups.0.NetworkInterface.InterfaceType",
                    json!("efa"),
                ),
                (
                    "InstanceGroups.1.InstanceRequirements.DesiredInstanceTypes.1",
                    json!("ml.m5.xlarge"),
                ),
                (
                    "InstanceGroups.1.InstanceRequirements.CurrentInstanceTypes.0",
                    json!("ml.m5.large"),
                ),
                ("InstanceGroups.1.CurrentCount", json!(3)),
            ],
            absent: vec!["InstanceGroups.1.InstanceType"],
        },
        RoundTrip {
            case: "restricted instance groups and their shared environment",
            request: json!({
                "InstanceGroups": [instance_group("workers", 1, "ml.m5.large", json!({}))],
                "RestrictedInstanceGroups": [{
                    "InstanceGroupName": "nova",
                    "InstanceCount": 2,
                    // A family the account holds capacity for: the accelerated generations
                    // carry a published quota of zero instances, which
                    // `admission_bounds` pins as its own rule.
                    "InstanceType": "ml.g5.48xlarge",
                    "ExecutionRole": EXECUTION_ROLE,
                    "EnvironmentConfig": {
                        "FSxLustreConfig": {
                            "SizeInGiB": 1200,
                            "PerUnitStorageThroughput": 125,
                        },
                    },
                }],
                "RestrictedInstanceGroupsConfig": {
                    "SharedEnvironmentConfig": {
                        "FSxLustreDeletionPolicy": "Keep",
                        "FSxLustreConfig": {
                            "SizeInGiB": 2400,
                            "PerUnitStorageThroughput": 250,
                        },
                    },
                },
            }),
            expected: vec![
                (
                    "RestrictedInstanceGroups.0.InstanceGroupName",
                    json!("nova"),
                ),
                ("RestrictedInstanceGroups.0.CurrentCount", json!(2)),
                ("RestrictedInstanceGroups.0.Status", json!("InService")),
                (
                    "RestrictedInstanceGroups.0.EnvironmentConfig.FSxLustreConfig.SizeInGiB",
                    json!(1200),
                ),
                (
                    "RestrictedInstanceGroupsConfig.SharedEnvironmentConfig.\
                     DesiredFSxLustreDeletionPolicy",
                    json!("Keep"),
                ),
                (
                    "RestrictedInstanceGroupsConfig.SharedEnvironmentConfig.\
                     CurrentFSxLustreConfig.PerUnitStorageThroughput",
                    json!(250),
                ),
            ],
            absent: vec!["RestrictedInstanceGroups.1"],
        },
        RoundTrip {
            case: "an EKS orchestrator with every cluster-wide option",
            request: json!({
                "InstanceGroups": [instance_group("workers", 1, "ml.m5.large", json!({}))],
                "Orchestrator": { "Eks": { "ClusterArn": EKS_CLUSTER_ARN } },
                "VpcConfig": {
                    "SecurityGroupIds": ["sg-0123456789abcdef0"],
                    "Subnets": ["subnet-0123456789abcdef0", "subnet-00000000000000001"],
                },
                "NodeRecovery": "Automatic",
                "NodeProvisioningMode": "Continuous",
                "ClusterRole": EXECUTION_ROLE,
                "AutoScaling": { "Mode": "Enable", "AutoScalerType": "Karpenter" },
                "TieredStorageConfig": {
                    "Mode": "Enable",
                    "InstanceMemoryAllocationPercentage": 40,
                },
                "Tags": [{ "Key": "team", "Value": "research" }],
            }),
            expected: vec![
                ("Orchestrator.Eks.ClusterArn", json!(EKS_CLUSTER_ARN)),
                ("VpcConfig.Subnets.1", json!("subnet-00000000000000001")),
                ("NodeRecovery", json!("Automatic")),
                ("NodeProvisioningMode", json!("Continuous")),
                ("ClusterRole", json!(EXECUTION_ROLE)),
                ("AutoScaling.Mode", json!("Enable")),
                ("AutoScaling.AutoScalerType", json!("Karpenter")),
                ("AutoScaling.Status", json!("InService")),
                ("TieredStorageConfig.InstanceMemoryAllocationPercentage", json!(40)),
            ],
            absent: vec!["Orchestrator.Slurm", "AutoScaling.FailureMessage"],
        },
        RoundTrip {
            case: "a Slurm orchestrator",
            request: json!({
                "InstanceGroups": [instance_group("workers", 1, "ml.m5.large", json!({}))],
                "Orchestrator": { "Slurm": { "SlurmConfigStrategy": "Managed" } },
                "NodeRecovery": "None",
            }),
            expected: vec![
                (
                    "Orchestrator.Slurm.SlurmConfigStrategy",
                    json!("Managed"),
                ),
                ("NodeRecovery", json!("None")),
            ],
            absent: vec!["Orchestrator.Eks"],
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("cluster-{index}");
        let arn = emulator
            .create_cluster(&named(&name, &case.request))
            .await;

        // The round trip is asserted once the clock owes `InService`, so the realised
        // half of every details shape is populated; the transient state has its own test.
        tokio::time::sleep(PAST_IN_SERVICE_DUE).await;

        // `ClusterName` is a `ClusterNameOrArn`: both forms must find the one record.
        let by_name = emulator.describe_cluster(&name).await;
        let by_arn = emulator.describe_cluster(&arn).await;
        assert_eq!(
            by_name, by_arn,
            "case {:?}: describing by name and by ARN must answer the same cluster, \
             got {by_name} and {by_arn}",
            case.case
        );
        assert_eq!(
            at(&by_name, "ClusterArn"),
            Some(&json!(arn)),
            "case {:?}: expected the created ARN {arn:?}, got {by_name}",
            case.case
        );
        assert_eq!(
            at(&by_name, "ClusterName"),
            Some(&json!(name)),
            "case {:?}: expected ClusterName {name:?}, got {by_name}",
            case.case
        );
        assert!(
            at(&by_name, "CreationTime").is_some(),
            "case {:?}: expected a CreationTime, got {by_name}",
            case.case
        );
        for (path, expected) in &case.expected {
            let actual = at(&by_name, path);
            assert_eq!(
                actual,
                Some(expected),
                "case {:?}: expected {path} = {expected}, got {actual:?} in {by_name}",
                case.case
            );
        }
        for path in &case.absent {
            let actual = at(&by_name, path);
            assert!(
                actual.is_none(),
                "case {:?}: expected {path} to be absent, got {actual:?} in {by_name}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn a_cluster_is_creating_with_nothing_realised_until_the_clock_owes_in_service() {
    // The counts span the modelled range's lower bound (`ClusterInstanceCount`,
    // range 0..6758) and two populated groups.
    for count in [0_i64, 1, 4] {
        let emulator = Emulator::new();
        let name = format!("provisioning-{count}");
        let body = named(
            &name,
            &json!({
                "InstanceGroups": [instance_group("workers", count, "ml.m5.large", json!({
                    "ImageId": "default",
                    "KubernetesConfig": { "Labels": { "team": "research" } },
                }))],
                "AutoScaling": { "Mode": "Enable" },
            }),
        );
        emulator.create_cluster(&body).await;

        // Straight after the create: provisioning, so nothing is realised yet.
        let creating = emulator.describe_cluster(&name).await;
        for (path, expected) in [
            ("ClusterStatus", json!("Creating")),
            ("InstanceGroups.0.Status", json!("Creating")),
            ("InstanceGroups.0.CurrentCount", json!(0)),
            ("InstanceGroups.0.TargetCount", json!(count)),
            ("InstanceGroups.0.DesiredImageId", json!("default")),
            ("AutoScaling.Status", json!("Creating")),
        ] {
            let actual = at(&creating, path);
            assert_eq!(
                actual,
                Some(&expected),
                "case count {count}: expected {path} = {expected} while creating, \
                 got {actual:?} in {creating}"
            );
        }
        for path in [
            "InstanceGroups.0.CurrentImageId",
            "InstanceGroups.0.KubernetesConfig.CurrentLabels",
        ] {
            let actual = at(&creating, path);
            assert!(
                actual.is_none(),
                "case count {count}: expected {path} to be absent while creating, \
                 got {actual:?} in {creating}"
            );
        }

        // The transition the clock owed, materialised by the next read.
        tokio::time::sleep(PAST_IN_SERVICE_DUE).await;
        let in_service = emulator.describe_cluster(&name).await;
        for (path, expected) in [
            ("ClusterStatus", json!("InService")),
            ("InstanceGroups.0.Status", json!("InService")),
            ("InstanceGroups.0.CurrentCount", json!(count)),
            ("InstanceGroups.0.CurrentImageId", json!("default")),
            (
                "InstanceGroups.0.KubernetesConfig.CurrentLabels.team",
                json!("research"),
            ),
            ("AutoScaling.Status", json!("InService")),
        ] {
            let actual = at(&in_service, path);
            assert_eq!(
                actual,
                Some(&expected),
                "case count {count}: expected {path} = {expected} once in service, \
                 got {actual:?} in {in_service}"
            );
        }
        // The transition rewrites the status, never the creation time, so a later read
        // reports the same instant.
        assert_eq!(
            at(&in_service, "CreationTime"),
            at(&creating, "CreationTime"),
            "case count {count}: CreationTime must be stable across reads, got \
             {in_service} after {creating}"
        );
        // And it is terminal: a further read does not move the cluster on again.
        let again = emulator.describe_cluster(&name).await;
        assert_eq!(
            again, in_service,
            "case count {count}: a settled cluster must read the same twice, got \
             {again} after {in_service}"
        );
    }
}

#[tokio::test]
async fn a_cluster_name_already_in_use_is_refused_as_resource_in_use() {
    let emulator = Emulator::new();
    let first = emulator
        .create_cluster(&named("twice", &one_group_body()))
        .await;

    // A second create under the same name, whatever else the request changes.
    for (case, body) in [
        ("the same request", one_group_body()),
        (
            "a different request under the same name",
            json!({ "InstanceGroups": [instance_group("other", 3, "ml.c5.large", json!({}))] }),
        ),
    ] {
        let (status, body) = emulator
            .call("CreateCluster", &named("twice", &body))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceInUse",
            "case {case:?}: expected ResourceInUse, got {body}"
        );
    }

    // The cluster that WAS created is untouched: the refusal wrote nothing.
    let described = emulator.describe_cluster("twice").await;
    assert_eq!(
        at(&described, "ClusterArn"),
        Some(&json!(first)),
        "expected the first cluster to survive the refusals, got {described}"
    );
    assert_eq!(
        at(&described, "InstanceGroups.0.InstanceGroupName"),
        Some(&json!("workers")),
        "expected the first cluster's own instance group, got {described}"
    );

    // And another name still works, so the refusal is about the name alone.
    let other = emulator
        .create_cluster(&named("twice-more", &one_group_body()))
        .await;
    assert_ne!(
        other, first,
        "expected a distinct cluster under a distinct name, got {other:?} twice"
    );
}

#[tokio::test]
async fn describe_cluster_refuses_an_unknown_identifier_with_resource_not_found() {
    let emulator = Emulator::new();
    let arn = emulator
        .create_cluster(&named("known", &one_group_body()))
        .await;
    let id = cluster_id(&arn).to_string();

    // Every row satisfies `ClusterNameOrArn`'s pattern, so the refusal comes from the
    // handler rather than from boundary validation.
    let cases = [
        ("an unknown name", "unknown".to_string()),
        (
            "a well-formed ARN no cluster carries",
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:cluster/aaaaaaaaaaaa"),
        ),
        (
            "a known ARN in another region",
            format!("arn:aws:sagemaker:eu-west-2:{ACCOUNT}:cluster/{id}"),
        ),
        ("a known cluster's id used as a name", id.clone()),
        ("a prefix of a known name", "know".to_string()),
    ];
    for (case, identifier) in cases {
        let (status, body) = emulator
            .call("DescribeCluster", &json!({ "ClusterName": identifier }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFound",
            "case {case:?}: expected ResourceNotFound, got {body}"
        );
    }

    // The known cluster is still readable by both of its identifiers.
    for identifier in ["known", arn.as_str()] {
        let described = emulator.describe_cluster(identifier).await;
        assert_eq!(
            at(&described, "ClusterArn"),
            Some(&json!(arn)),
            "case {identifier:?}: expected the known cluster, got {described}"
        );
    }
}

#[tokio::test]
async fn orchestrator_must_name_exactly_one_configuration() {
    // `CreateClusterRequest$Orchestrator`'s doc: "If you specify the Orchestrator field,
    // you must provide exactly one orchestrator configuration: either Eks or Slurm.
    // Specifying both or providing an empty configuration returns a validation error."
    let cases = [
        ("an empty configuration", Some(json!({})), false),
        (
            "both configurations",
            Some(json!({
                "Eks": { "ClusterArn": EKS_CLUSTER_ARN },
                "Slurm": { "SlurmConfigStrategy": "Managed" },
            })),
            false,
        ),
        (
            "Eks alone",
            Some(json!({ "Eks": { "ClusterArn": EKS_CLUSTER_ARN } })),
            true,
        ),
        (
            "Slurm alone",
            Some(json!({ "Slurm": { "SlurmConfigStrategy": "Merge" } })),
            true,
        ),
        ("no Orchestrator member at all", None, true),
    ];

    for (index, (case, orchestrator, accepted)) in cases.into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("orchestrated-{index}");
        let mut body = named(&name, &one_group_body());
        if let Some(orchestrator) = orchestrator {
            body.as_object_mut()
                .expect("a JSON object")
                .insert("Orchestrator".to_string(), orchestrator);
        }
        let (status, response) = emulator.call("CreateCluster", &body).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {response}"
            );
            let described = emulator.describe_cluster(&name).await;
            assert!(
                at(&described, "ClusterArn").is_some(),
                "case {case:?}: expected the cluster to be readable, got {described}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {response}"
            );
            let (status, refused) = emulator
                .call("DescribeCluster", &json!({ "ClusterName": name }))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: a refused create must persist nothing, got {status} \
                 with {refused}"
            );
        }
    }
}

