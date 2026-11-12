// SPDX-License-Identifier: CC0-1.0

//! What an Amazon EKS cluster's control plane logging does NOT export, across the two
//! services that have to agree for a client to see it.
//!
//! RULE: `LogSetup$enabled` is "If a log type is enabled, that log type exports its
//! control plane logs to CloudWatch Logs. If a log type isn't enabled, that log type
//! doesn't export its control plane logs. Each individual log type can be enabled or
//! disabled independently" — so a type nobody enabled exports NOTHING, and a cluster whose
//! configuration enables no type at all has no log group of its own
//! (`/aws/eks/<cluster name>/cluster`) and no stream under it. That is what makes an
//! enabled type observably different from a disabled one, and it holds whichever call set
//! the configuration — `CreateCluster` or `UpdateClusterConfig` — and whatever else an
//! update changes.
//!
//! The other half of the rule — what an ENABLED type exports — is pinned by
//! `eks_control_plane_logs.rs` beside this file. This suite pins the negative half, which
//! a client can only observe as an absence, so every row that asserts an absence is read
//! BESIDE a cluster of the same account that does export: a log group missing because the
//! service exported nothing is only meaningful when the service's export is working.
//!
//! The rows differ along what the rule generalises over: the FORM of a configuration that
//! enables nothing (every type named and switched off, a subset switched off, an enabling
//! entry naming no type, no entry at all, no `logging` member), WHICH call carries it
//! (create or update), and WHAT ELSE an update changes (a logging-free update changes no
//! export at all).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request here is signed for.
const REGION: &str = "us-west-2";

/// The two signing names, which is how the router picks the service that serves each
/// request.
const EKS: &str = "eks";
const LOGS: &str = "logs";

/// The `X-Amz-Target` prefix CloudWatch Logs names an operation under: its service shape's
/// name. (EKS is `restJson1` and carries none.)
const LOGS_TARGET_PREFIX: &str = "Logs_20140328";

/// Every member of `LogType`, as the EKS model declares them.
const EVERY_LOG_TYPE: &[&str] = &[
    "api",
    "audit",
    "authenticator",
    "controllerManager",
    "scheduler",
];

/// How long this suite waits between polls for a cluster's owed `CREATING` -> `ACTIVE` (or
/// `UPDATING` -> `ACTIVE`) move to fall due.
const POLL_WAIT: Duration = Duration::from_millis(100);

/// How many times it polls for that move, which bounds the wait.
const POLL_ATTEMPTS: usize = 60;

/// The two services over ONE shared engine, as `router/src/lib.rs` mounts them.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These rows exercise handler behaviour, not request signing.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_eks_awswesleyfrontend::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn dispatch(
        &self,
        signing_name: &str,
        method: &str,
        uri: &str,
        target: Option<String>,
        body: Value,
    ) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{signing_name}/aws4_request, \
             SignedHeaders=host, Signature=0000"
        );
        let mut request = http::Request::builder()
            .method(method)
            .uri(format!("https://localhost:9297{uri}"))
            .header("host", "localhost:9297")
            .header("authorization", authorization)
            .header("content-type", "application/x-amz-json-1.1");
        if let Some(target) = target {
            request = request.header("x-amz-target", target);
        }
        let response = self
            .router
            .dispatch(
                request
                    .body(Bytes::from(body.to_string()))
                    .expect("a well-formed request"),
            )
            .await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let parsed = if raw.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One EKS call (restJson1), with the status it answered.
    async fn eks(&self, method: &str, uri: &str, body: Value) -> (u16, Value) {
        self.dispatch(EKS, method, uri, None, body).await
    }

    /// One CloudWatch Logs call that must succeed.
    async fn logs(&self, operation: &str, body: Value, case: &str) -> Value {
        let (status, parsed) = self
            .dispatch(
                LOGS,
                "POST",
                "/",
                Some(format!("{LOGS_TARGET_PREFIX}.{operation}")),
                body,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected logs {operation} to answer 200, got {status} with {parsed}"
        );
        parsed
    }

    /// Create a cluster, with the `logging` the row carries (when it carries one).
    async fn create_cluster(&self, cluster: &str, logging: Option<Value>, case: &str) {
        let mut body = json!({
            "name": cluster,
            "roleArn": format!("arn:aws:iam::{}:role/eksClusterRole", engine::emulator_account_id()),
            "resourcesVpcConfig": { "subnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"] },
        });
        if let Some(logging) = logging {
            body["logging"] = logging;
        }
        let (status, parsed) = self.eks("POST", "/clusters", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the cluster must be created, got {status} with {parsed}"
        );
    }

    /// The cluster as `DescribeCluster` reports it, once the clock's owed move to `ACTIVE`
    /// has been materialised by a read — which is also what `UpdateClusterConfig` needs
    /// before it accepts the next change.
    async fn cluster_when_active(&self, cluster: &str, case: &str) -> Value {
        for _ in 0..POLL_ATTEMPTS {
            let (status, parsed) = self
                .eks("GET", &format!("/clusters/{cluster}"), json!({}))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the cluster must be readable, got {status} with {parsed}"
            );
            if parsed["cluster"]["status"] == json!("ACTIVE") {
                return parsed["cluster"].clone();
            }
            tokio::time::sleep(POLL_WAIT).await;
        }
        panic!("case {case:?}: the cluster never reached ACTIVE");
    }

    /// One `UpdateClusterConfig` call, which must be accepted and report `expected_type`.
    async fn update_config(&self, cluster: &str, body: Value, expected_type: &str, case: &str) {
        let (status, parsed) = self
            .eks(
                "POST",
                &format!("/clusters/{cluster}/update-config"),
                body.clone(),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateClusterConfig {body} must answer 200, got {status} with \
             {parsed}"
        );
        assert_eq!(
            parsed["update"]["type"],
            json!(expected_type),
            "case {case:?}: expected a {expected_type}, got {parsed}"
        );
    }

    /// Whether CloudWatch Logs holds the cluster's log group, as a client finds it:
    /// `DescribeLogGroups` narrowed to that name.
    async fn holds_log_group(&self, cluster: &str, case: &str) -> bool {
        let group = cluster_log_group(cluster);
        let answer = self
            .logs(
                "DescribeLogGroups",
                json!({ "logGroupNamePrefix": &group }),
                case,
            )
            .await;
        answer["logGroups"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .any(|held| held["logGroupName"] == json!(group))
    }

    /// The log streams the cluster's group holds, by name.
    async fn log_streams(&self, cluster: &str, case: &str) -> Vec<String> {
        let answer = self
            .logs(
                "DescribeLogStreams",
                json!({ "logGroupName": cluster_log_group(cluster) }),
                case,
            )
            .await;
        answer["logStreams"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|stream| stream["logStreamName"].as_str().map(str::to_owned))
            .collect()
    }
}

/// The log group a cluster's control plane logs are exported to.
fn cluster_log_group(cluster: &str) -> String {
    format!("/aws/eks/{cluster}/cluster")
}

/// One `logging` payload enabling `types`, as a client writes it.
fn enabling(types: &[&str]) -> Value {
    json!({ "clusterLogging": [{ "types": types, "enabled": true }] })
}

/// One `logging` payload disabling `types`.
fn disabling(types: &[&str]) -> Value {
    json!({ "clusterLogging": [{ "types": types, "enabled": false }] })
}

/// The types a cluster REPORTS as exporting, read off the `LogSetup` groups
/// `Cluster$logging` carries.
fn reported_enabled_types(cluster: &Value) -> Vec<String> {
    cluster["logging"]["clusterLogging"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|setup| setup["enabled"] == json!(true))
        .flat_map(|setup| {
            setup["types"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect::<Vec<String>>()
        })
        .collect()
}

/// Assert that CloudWatch Logs holds nothing at all for this cluster: no group, so no
/// stream and no record either.
async fn assert_no_export(emulator: &Emulator, cluster: &str, case: &str) {
    assert!(
        !emulator.holds_log_group(cluster, case).await,
        "case {case:?}: a cluster that enables no log type must have no log group, got {}",
        cluster_log_group(cluster)
    );
}

/// Assert that the cluster's group holds exactly `expected` streams, by name.
async fn assert_streams(emulator: &Emulator, cluster: &str, expected: &[&str], case: &str) {
    assert!(
        emulator.holds_log_group(cluster, case).await,
        "case {case:?}: CloudWatch Logs must hold {}, got none",
        cluster_log_group(cluster)
    );
    let streams = emulator.log_streams(cluster, case).await;
    assert_eq!(
        streams.len(),
        expected.len(),
        "case {case:?}: expected exactly the streams {expected:?}, got {streams:?}"
    );
    for component in expected {
        let prefix = format!("{component}-");
        let found: Vec<&String> = streams
            .iter()
            .filter(|stream| {
                stream.strip_prefix(prefix.as_str()).is_some_and(|suffix| {
                    // `kube-apiserver-audit-<suffix>` also starts with `kube-apiserver-`,
                    // so a stream only belongs to this component when what follows the
                    // component's name is the suffix itself.
                    !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            })
            .collect();
        assert_eq!(
            found.len(),
            1,
            "case {case:?}: expected exactly one {component:?} stream, got {streams:?}"
        );
    }
}

/// One row of the create-time table: a `logging` a create carries, and whether the cluster
/// it creates exports anything at all.
struct CreateRow {
    case: &'static str,
    cluster: &'static str,
    /// The `logging` member the create carries, or `None` for a create that carries none.
    logging: Option<Value>,
    /// The types the configuration enables, in the order the model declares them — what
    /// the cluster must report as its enabled group.
    enabled: Vec<&'static str>,
    /// The streams the cluster's group must hold, named after the control plane component
    /// of each exporting type — EMPTY for a configuration that enables nothing, which has
    /// no group at all.
    streams: Vec<&'static str>,
}

#[tokio::test]
async fn a_configuration_that_enables_no_log_type_exports_nothing() {
    let emulator = Emulator::new();

    let rows = vec![
        CreateRow {
            case: "every type named and switched off",
            cluster: "cp-off-every-type",
            logging: Some(disabling(EVERY_LOG_TYPE)),
            enabled: vec![],
            streams: vec![],
        },
        CreateRow {
            case: "one type switched off, the rest not named at all",
            cluster: "cp-off-one-type",
            logging: Some(disabling(&["api"])),
            enabled: vec![],
            streams: vec![],
        },
        CreateRow {
            case: "two entries, each switching its own types off",
            cluster: "cp-off-two-entries",
            logging: Some(json!({
                "clusterLogging": [
                    { "types": ["scheduler"], "enabled": false },
                    { "types": ["audit", "authenticator"], "enabled": false },
                ],
            })),
            enabled: vec![],
            streams: vec![],
        },
        CreateRow {
            case: "an enabling entry that names no type",
            cluster: "cp-off-no-type-named",
            logging: Some(enabling(&[])),
            enabled: vec![],
            streams: vec![],
        },
        CreateRow {
            case: "a configuration with no entry at all",
            cluster: "cp-off-no-entry",
            logging: Some(json!({ "clusterLogging": [] })),
            enabled: vec![],
            streams: vec![],
        },
        CreateRow {
            case: "a logging member carrying nothing",
            cluster: "cp-off-empty-logging",
            logging: Some(json!({})),
            enabled: vec![],
            streams: vec![],
        },
        CreateRow {
            case: "no logging member at all",
            cluster: "cp-off-absent-logging",
            logging: None,
            enabled: vec![],
            streams: vec![],
        },
        // The control rows: the same account, the same call, one type switched ON — so
        // every absence above is read beside an export that works. They name types the
        // rows of `eks_control_plane_logs.rs` do not lead with.
        CreateRow {
            case: "one type switched on beside the types switched off",
            cluster: "cp-off-control-mixed",
            logging: Some(json!({
                "clusterLogging": [
                    { "types": ["controllerManager"], "enabled": true },
                    { "types": ["api", "audit", "authenticator", "scheduler"], "enabled": false },
                ],
            })),
            enabled: vec!["controllerManager"],
            streams: vec!["kube-controller-manager"],
        },
        CreateRow {
            case: "two types switched on, the rest never named",
            cluster: "cp-off-control-pair",
            logging: Some(enabling(&["authenticator", "scheduler"])),
            enabled: vec!["authenticator", "scheduler"],
            streams: vec!["authenticator", "kube-scheduler"],
        },
    ];

    for row in rows {
        emulator
            .create_cluster(row.cluster, row.logging.clone(), row.case)
            .await;
        if row.streams.is_empty() {
            assert_no_export(&emulator, row.cluster, row.case).await;
        } else {
            assert_streams(&emulator, row.cluster, &row.streams, row.case).await;
        }

        // What the cluster REPORTS agrees with what it exports: the types it reports as
        // enabled are exactly the types that have a stream.
        let reported =
            reported_enabled_types(&emulator.cluster_when_active(row.cluster, row.case).await);
        assert_eq!(
            reported, row.enabled,
            "case {:?}: the cluster must report {:?} as exporting, got {reported:?}",
            row.case, row.enabled
        );
        assert_eq!(
            reported.len(),
            row.streams.len(),
            "case {:?}: the cluster must report one enabled type per exporting stream \
             {:?}, got {reported:?}",
            row.case,
            row.streams
        );
    }
}

#[tokio::test]
async fn switching_every_type_off_exports_nothing_and_keeps_what_already_exported() {
    let emulator = Emulator::new();
    let case = "types switched off, then on, then off again";
    let cluster = "cp-off-update";

    // A cluster created without logging exports nothing...
    emulator.create_cluster(cluster, None, case).await;
    assert_no_export(&emulator, cluster, case).await;

    // ...and switching ONE type on is what creates the group, with that type's stream and
    // no other: the four types left off have none. The cluster had no group of its own
    // until this call, so turning a type on is what provisioned it.
    emulator.cluster_when_active(cluster, case).await;
    emulator
        .update_config(
            cluster,
            json!({ "logging": enabling(&["audit"]) }),
            "LoggingUpdate",
            case,
        )
        .await;
    assert_streams(&emulator, cluster, &["kube-apiserver-audit"], case).await;

    // Switching it off again changes the cluster's configuration and nothing in
    // CloudWatch Logs: the records already exported belong to the account, so the group
    // and that one stream stay, and no stream appears for a type that is now off.
    let active = emulator.cluster_when_active(cluster, case).await;
    assert_eq!(
        reported_enabled_types(&active),
        vec!["audit".to_string()],
        "case {case:?}: the cluster must report the type it exports, got {}",
        active["logging"]
    );
    emulator
        .update_config(
            cluster,
            json!({ "logging": disabling(EVERY_LOG_TYPE) }),
            "LoggingUpdate",
            case,
        )
        .await;
    assert_streams(&emulator, cluster, &["kube-apiserver-audit"], case).await;
    let active = emulator.cluster_when_active(cluster, case).await;
    assert!(
        reported_enabled_types(&active).is_empty(),
        "case {case:?}: the cluster must report no type as exporting, got {}",
        active["logging"]
    );

    // And an update that carries no `logging` at all exports nothing of its own: the
    // export follows the logging configuration, not any change to the cluster.
    emulator
        .update_config(
            cluster,
            json!({ "resourcesVpcConfig": { "endpointPrivateAccess": true } }),
            "EndpointAccessUpdate",
            case,
        )
        .await;
    assert_streams(&emulator, cluster, &["kube-apiserver-audit"], case).await;

    // The same logging-free update on a cluster that never enabled a type leaves it with
    // no group at all.
    let untouched = "cp-off-update-untouched";
    emulator.create_cluster(untouched, None, case).await;
    emulator.cluster_when_active(untouched, case).await;
    emulator
        .update_config(
            untouched,
            json!({ "resourcesVpcConfig": { "endpointPrivateAccess": true } }),
            "EndpointAccessUpdate",
            case,
        )
        .await;
    assert_no_export(&emulator, untouched, case).await;
}
