// SPDX-License-Identifier: CC0-1.0

//! The WORKFLOW a client follows to turn an Amazon EKS cluster's control plane logging on:
//! `UpdateClusterConfig` carrying a `logging` configuration, the update id it answers with
//! polled through `DescribeUpdate` until it reports `Successful`, and only then the logs
//! themselves read out of CloudWatch Logs.
//!
//! `LogSetup$enabled` is "If a log type is enabled, that log type exports its control plane
//! logs to CloudWatch Logs. If a log type isn't enabled, that log type doesn't export its
//! control plane logs. Each individual log type can be enabled or disabled independently",
//! so the one externally visible effect of the setting lives in CloudWatch Logs: the log
//! group `/aws/eks/<cluster name>/cluster`, which the service provisions itself, and one
//! stream per exporting type carrying that type's records. A client that enables a type,
//! sees its update reach `Successful` and then finds nothing in CloudWatch Logs cannot tell
//! a logging cluster from a silent one.
//!
//! The rule these rows pin: once the update a `logging` change started reports `Successful`,
//! the cluster reports exactly the types that configuration enables AND CloudWatch Logs
//! serves `/aws/eks/<cluster name>/cluster` — found the way a client finds it, through
//! `DescribeLogGroups` on that prefix — holding one record-carrying stream per enabled type
//! and none for a type the configuration leaves off.
//!
//! The rows differ along how many types one call enables (none, one, two, three, every
//! declared type), WHICH types they are (each member of `LogType` is enabled by some row),
//! the shape of the configuration carrying them (one entry, two entries, an enabling entry
//! beside a disabling one), and whether the same configuration is applied once or twice.
//! Walking to `Successful` is what the suites beside this one (`eks_control_plane_logs.rs`,
//! `eks_control_plane_log_suppression.rs`) do not do: they read the export straight after
//! the accepted call. The harness is this suite's own, as theirs are: `eks_wire.rs` drives
//! only the protocols those suites need and carries no `X-Amz-Target` driver, which is how
//! CloudWatch Logs is reached.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request here is signed for.
const REGION: &str = "us-west-2";

/// The two signing names, which is how the router picks the service serving each request.
const EKS: &str = "eks";
const LOGS: &str = "logs";

/// The `X-Amz-Target` prefix CloudWatch Logs names an operation under: its service shape's
/// name. (EKS is `restJson1` and carries none.)
const LOGS_TARGET_PREFIX: &str = "Logs_20140328";

/// The `UpdateStatus` an update still running reports, and the terminal one a configuration
/// change that was applied settles on (`smithy_query(query="shape",shape="UpdateStatus")`).
const UPDATE_IN_PROGRESS: &str = "InProgress";
const UPDATE_SUCCESSFUL: &str = "Successful";

/// The control plane component each log type's streams are named after, as the Amazon EKS
/// control plane logging documentation publishes them — one entry per member of `LogType`
/// (`smithy_query(query="shape",shape="LogType")`).
const COMPONENT_OF_LOG_TYPE: &[(&str, &str)] = &[
    ("api", "kube-apiserver"),
    ("audit", "kube-apiserver-audit"),
    ("authenticator", "authenticator"),
    ("controllerManager", "kube-controller-manager"),
    ("scheduler", "kube-scheduler"),
];

/// How long to wait between two looks at a condition the clock owes (a cluster's
/// `CREATING` -> `ACTIVE` move, an update's completion).
const POLL_WAIT: Duration = Duration::from_millis(100);

/// How many looks bound such a wait. Generous on purpose: the condition, never the clock, is
/// what these rows assert on.
const POLL_ATTEMPTS: usize = 200;

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

    /// One EKS call (restJson1) that must succeed.
    async fn eks(&self, method: &str, uri: &str, body: Value, case: &str) -> Value {
        let (status, parsed) = self.dispatch(EKS, method, uri, None, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected eks {method} {uri} to answer 200, got {status} with {parsed}"
        );
        parsed
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

    /// Create a cluster carrying NO logging configuration — the starting point of the
    /// turn-it-on-afterwards workflow — and wait for it to report `ACTIVE`.
    async fn create_active_cluster(&self, cluster: &str, case: &str) {
        let body = json!({
            "name": cluster,
            "roleArn": format!("arn:aws:iam::{}:role/eksClusterRole", engine::emulator_account_id()),
            "resourcesVpcConfig": { "subnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"] },
        });
        self.eks("POST", "/clusters", body, case).await;
        self.wait_until_active(cluster, case).await;
    }

    /// Wait for the cluster to report `ACTIVE`, the status `UpdateClusterConfig` needs — the
    /// status a create, and the completion of an earlier update, each owe it.
    async fn wait_until_active(&self, cluster: &str, case: &str) {
        for _ in 0..POLL_ATTEMPTS {
            if self.describe_cluster(cluster, case).await["status"] == json!("ACTIVE") {
                return;
            }
            tokio::time::sleep(POLL_WAIT).await;
        }
        panic!("case {case:?}: the cluster never reached ACTIVE");
    }

    /// The cluster as `DescribeCluster` reports it.
    async fn describe_cluster(&self, cluster: &str, case: &str) -> Value {
        self.eks("GET", &format!("/clusters/{cluster}"), json!({}), case)
            .await["cluster"]
            .clone()
    }

    /// Start a `logging` change and answer the id of the update it minted.
    async fn update_logging(&self, cluster: &str, logging: Value, case: &str) -> String {
        let answer = self
            .eks(
                "POST",
                &format!("/clusters/{cluster}/update-config"),
                json!({ "logging": logging }),
                case,
            )
            .await;
        assert_eq!(
            answer["update"]["type"],
            json!("LoggingUpdate"),
            "case {case:?}: a logging change is a LoggingUpdate, got {answer}"
        );
        answer["update"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the update must carry an id, got {answer}"))
            .to_string()
    }

    /// Poll `DescribeUpdate` until the update leaves `InProgress` and answer the status it
    /// settled on — the step a client performs before it goes looking for the logs.
    async fn update_status_when_settled(
        &self,
        cluster: &str,
        update_id: &str,
        case: &str,
    ) -> String {
        for _ in 0..POLL_ATTEMPTS {
            let answer = self
                .eks(
                    "GET",
                    &format!("/clusters/{cluster}/updates/{update_id}"),
                    json!({}),
                    case,
                )
                .await;
            let status = answer["update"]["status"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if status != UPDATE_IN_PROGRESS {
                return status;
            }
            tokio::time::sleep(POLL_WAIT).await;
        }
        panic!("case {case:?}: update {update_id} never left {UPDATE_IN_PROGRESS}");
    }

    /// Whether CloudWatch Logs serves the cluster's log group, found as a client finds it:
    /// `DescribeLogGroups` narrowed to that name as a prefix.
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

    /// The streams the cluster's log group holds, by name.
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

    /// The messages one stream of the cluster's log group holds.
    async fn log_events(&self, cluster: &str, stream: &str, case: &str) -> Vec<String> {
        let answer = self
            .logs(
                "GetLogEvents",
                json!({
                    "logGroupName": cluster_log_group(cluster),
                    "logStreamName": stream,
                }),
                case,
            )
            .await;
        answer["events"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|event| event["message"].as_str().map(str::to_owned))
            .collect()
    }
}

/// The log group a cluster's control plane logs are exported to.
fn cluster_log_group(cluster: &str) -> String {
    format!("/aws/eks/{cluster}/cluster")
}

/// One `logging` payload enabling `types` in a single entry, as a client writes it.
fn enabling(types: &[&str]) -> Value {
    json!({ "clusterLogging": [{ "types": types, "enabled": true }] })
}

/// The component a log type's stream is named after.
fn component_of(log_type: &str) -> &'static str {
    COMPONENT_OF_LOG_TYPE
        .iter()
        .find(|(declared, _)| *declared == log_type)
        .map(|(_, component)| *component)
        .unwrap_or_else(|| panic!("{log_type:?} is not a declared log type"))
}

/// The component one stream belongs to: the LONGEST published component name the stream is
/// that component's name followed by a suffix. The longest, because one component's name is
/// a prefix of another's (`kube-apiserver` of `kube-apiserver-audit`).
fn component_of_stream(stream: &str) -> Option<&'static str> {
    COMPONENT_OF_LOG_TYPE
        .iter()
        .map(|(_, component)| *component)
        .filter(|component| {
            stream
                .strip_prefix(component)
                .is_some_and(|suffix| suffix.starts_with('-'))
        })
        .max_by_key(|component| component.len())
}

/// The log types one cluster reports as enabled, in the order it reports them.
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
                .filter_map(|named| named.as_str().map(str::to_owned))
                .collect::<Vec<String>>()
        })
        .collect()
}

/// Assert that the cluster's group is served and holds exactly one record-carrying stream
/// per exporting type, and nothing else.
async fn assert_exports(emulator: &Emulator, cluster: &str, exporting: &[&str], case: &str) {
    assert!(
        emulator.holds_log_group(cluster, case).await,
        "case {case:?}: CloudWatch Logs must serve {}, got no such group",
        cluster_log_group(cluster)
    );
    let streams = emulator.log_streams(cluster, case).await;
    assert_eq!(
        streams.len(),
        exporting.len(),
        "case {case:?}: expected one stream per exporting type {exporting:?}, got {streams:?}"
    );
    for log_type in exporting {
        let component = component_of(log_type);
        let found: Vec<&String> = streams
            .iter()
            .filter(|stream| component_of_stream(stream) == Some(component))
            .collect();
        assert_eq!(
            found.len(),
            1,
            "case {case:?}: {log_type:?} must export under exactly one {component:?} stream, \
             got {streams:?}"
        );
        let events = emulator.log_events(cluster, found[0], case).await;
        assert!(
            !events.is_empty(),
            "case {case:?}: {log_type:?}'s stream {:?} must hold the exported record, got \
             {events:?}",
            found[0]
        );
    }
}

/// Assert that nothing was created for a cluster that exports no type.
async fn assert_no_export(emulator: &Emulator, cluster: &str, case: &str) {
    assert!(
        !emulator.holds_log_group(cluster, case).await,
        "case {case:?}: a cluster that exports no log type must have no log group, got {}",
        cluster_log_group(cluster)
    );
}

/// One row: the `logging` a client turns on through `UpdateClusterConfig`, and the types that
/// must then be exporting.
struct Row {
    case: &'static str,
    cluster: &'static str,
    logging: Value,
    /// The types the configuration enables, in the order the model declares them.
    exporting: Vec<&'static str>,
}

#[tokio::test]
async fn a_logging_update_that_reports_successful_has_exported_its_types() {
    let emulator = Emulator::new();

    let rows = vec![
        Row {
            case: "one type enabled",
            cluster: "logging-update-api",
            logging: enabling(&["api"]),
            exporting: vec!["api"],
        },
        Row {
            case: "two types enabled in one entry",
            cluster: "logging-update-api-audit",
            logging: enabling(&["api", "audit"]),
            exporting: vec!["api", "audit"],
        },
        Row {
            case: "a type the report did not cite, enabled on its own",
            cluster: "logging-update-controller",
            logging: enabling(&["controllerManager"]),
            exporting: vec!["controllerManager"],
        },
        Row {
            case: "three types enabled across two entries",
            cluster: "logging-update-three",
            logging: json!({
                "clusterLogging": [
                    { "types": ["authenticator"], "enabled": true },
                    { "types": ["scheduler", "audit"], "enabled": true },
                ],
            }),
            exporting: vec!["audit", "authenticator", "scheduler"],
        },
        Row {
            case: "one type enabled beside an entry disabling the rest",
            cluster: "logging-update-mixed",
            logging: json!({
                "clusterLogging": [
                    { "types": ["scheduler"], "enabled": true },
                    {
                        "types": ["api", "audit", "authenticator", "controllerManager"],
                        "enabled": false,
                    },
                ],
            }),
            exporting: vec!["scheduler"],
        },
        Row {
            case: "every declared type enabled at once",
            cluster: "logging-update-every-type",
            logging: enabling(&[
                "api",
                "audit",
                "authenticator",
                "controllerManager",
                "scheduler",
            ]),
            exporting: vec![
                "api",
                "audit",
                "authenticator",
                "controllerManager",
                "scheduler",
            ],
        },
        Row {
            case: "a configuration that enables no type at all",
            cluster: "logging-update-none",
            logging: json!({
                "clusterLogging": [{
                    "types": ["api", "audit", "authenticator", "controllerManager", "scheduler"],
                    "enabled": false,
                }],
            }),
            exporting: vec![],
        },
    ];

    for row in rows {
        let case = row.case;
        emulator.create_active_cluster(row.cluster, case).await;
        // Nothing exports before the change: the create carried no logging at all.
        assert_no_export(&emulator, row.cluster, case).await;

        let update_id = emulator
            .update_logging(row.cluster, row.logging.clone(), case)
            .await;
        let status = emulator
            .update_status_when_settled(row.cluster, &update_id, case)
            .await;
        assert_eq!(
            status, UPDATE_SUCCESSFUL,
            "case {case:?}: the logging update must settle on {UPDATE_SUCCESSFUL}, got {status:?}"
        );

        // What the cluster reports and what CloudWatch Logs serves agree, and both are the
        // types the configuration enabled.
        let reported = reported_enabled_types(&emulator.describe_cluster(row.cluster, case).await);
        let expected: Vec<String> = row
            .exporting
            .iter()
            .map(|log_type| (*log_type).to_string())
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: the cluster must report {expected:?} as enabled, got {reported:?}"
        );
        if row.exporting.is_empty() {
            assert_no_export(&emulator, row.cluster, case).await;
        } else {
            assert_exports(&emulator, row.cluster, &row.exporting, case).await;
        }
    }
}

#[tokio::test]
async fn applying_the_same_logging_configuration_again_adds_no_second_stream() {
    let emulator = Emulator::new();
    let case = "the same type enabled twice over";
    let cluster = "logging-update-repeated";

    emulator.create_active_cluster(cluster, case).await;
    for _ in 0..2 {
        // The second pass needs the cluster back out of `UPDATING`, which the completion of
        // the first update owes it.
        emulator.wait_until_active(cluster, case).await;
        let update_id = emulator
            .update_logging(cluster, enabling(&["api"]), case)
            .await;
        let status = emulator
            .update_status_when_settled(cluster, &update_id, case)
            .await;
        assert_eq!(
            status, UPDATE_SUCCESSFUL,
            "case {case:?}: each logging update must settle on {UPDATE_SUCCESSFUL}, got \
             {status:?}"
        );
        // One group, and one stream per type however many times that type is turned on: a
        // later export appends to the stream the earlier one opened.
        assert_exports(&emulator, cluster, &["api"], case).await;
    }
}
