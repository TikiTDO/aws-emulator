// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EKS cluster-configuration slice: `UpdateClusterConfig` and
//! the `DescribeCluster`/`DescribeUpdate` readers its update is read back through,
//! driven over the wire through the same `MultiServiceRouter` the production binary
//! mounts.
//!
//! Every suite asserts a RULE over a TABLE of inputs rather than one recorded request.
//! The rule under test: a configuration change starts ONE asynchronous update whose
//! `type` is the CATEGORY of configuration the request changed and whose `params` report
//! the members it carried; the cluster keeps its old configuration and reports
//! `UPDATING` until the clock completes the update, and only then reports the requested
//! values and `ACTIVE`. The rows differ along which category is changed, how many
//! members of it the request carries, and how many categories one request spans; each
//! assertion names its case and prints the actual value.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    ComAmazonawsEksAwswesleyfrontendState, UpdateRecord, UpdateScope, CLUSTER_ACTIVATION_DELAY,
    MAX_UPDATES_PER_CLUSTER, UPDATE_COMPLETION_DELAY, UPDATE_SUCCESSFUL,
};
use service_com_amazonaws_eks_awswesleyfrontend::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answer from the emulator, reduced to what a client branches on.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// The restJson1 error envelope's `__type`, absent on success.
    error_type: Option<String>,
    body: serde_json::Value,
}

impl Answer {
    fn cluster(&self) -> &serde_json::Value {
        &self.body["cluster"]
    }

    fn update(&self) -> &serde_json::Value {
        &self.body["update"]
    }

    fn update_id(&self) -> String {
        self.update()["id"].as_str().unwrap_or_default().to_owned()
    }

    /// The `UpdateParamType` keys the update reports, sorted so a case can compare
    /// them as a set.
    fn param_types(&self) -> Vec<String> {
        let mut types: Vec<String> = self.update()["params"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|param| param["type"].as_str().unwrap_or_default().to_owned())
            .collect();
        types.sort();
        types
    }

    /// The value the update reports for one `UpdateParamType`, parsed back out of the
    /// string `UpdateParam$value` carries.
    fn param_value(&self, param_type: &str) -> serde_json::Value {
        self.update()["params"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .find(|param| param["type"] == serde_json::json!(param_type))
            .and_then(|param| param["value"].as_str().map(str::to_owned))
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or(serde_json::Value::Null)
    }
}

/// The emulator under test: one shared engine, one service, authentication off so a
/// test signs nothing (the credential scope below is still parsed for its region).
fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, with the engine handle a suite needs to seed a precondition
/// directly into the store the handlers read.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: &str,
) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/eks/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(body.to_owned()))
        .expect("a well-formed test request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = response.body();
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(bytes).unwrap_or(serde_json::Value::Null)
    };
    let error_type = error_type.or_else(|| {
        body.get("__type")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    });
    Answer {
        status,
        error_type,
        body,
    }
}

const REGION: &str = "us-west-2";

/// A role of the single account this emulator serves, which is what
/// `iam:PassRole` allows.
const PASSABLE_ROLE: &str = "arn:aws:iam::000000000000:role/eksClusterRole";

/// The smallest `CreateCluster` request this service accepts, plus whatever `extra`
/// members a row needs on the created cluster.
fn create_request(name: &str, extra: serde_json::Value) -> serde_json::Value {
    let mut request = serde_json::json!({
        "name": name,
        "roleArn": PASSABLE_ROLE,
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    });
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        request[key] = value;
    }
    request
}

async fn create_with(router: &MultiServiceRouter, name: &str, extra: serde_json::Value) -> Answer {
    call(
        router,
        REGION,
        "POST",
        "/clusters",
        &create_request(name, extra).to_string(),
    )
    .await
}

async fn create(router: &MultiServiceRouter, name: &str) -> Answer {
    create_with(router, name, serde_json::json!({})).await
}

async fn describe(router: &MultiServiceRouter, name: &str) -> Answer {
    call(router, REGION, "GET", &format!("/clusters/{name}"), "").await
}

async fn describe_update(router: &MultiServiceRouter, name: &str, update_id: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("/clusters/{name}/updates/{update_id}"),
        "",
    )
    .await
}

async fn update_config(
    router: &MultiServiceRouter,
    name: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{name}/update-config"),
        &body.to_string(),
    )
    .await
}

/// Wait out the span the clock owes a newly created cluster before it is `ACTIVE`,
/// plus a margin so the test does not race the due instant.
async fn settle_creation() {
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;
}

/// Wait out the span the clock owes an in-flight update before it is `Successful`.
async fn settle_update() {
    tokio::time::sleep(UPDATE_COMPLETION_DELAY + Duration::from_millis(250)).await;
}

/// `true` when `id` has the shape of a version-4 UUID, which is what `Update$id` is
/// documented to be ("a UUID that is used to track the update").
fn is_uuid_v4(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    let [a, b, c, d, e] = groups.as_slice() else {
        return false;
    };
    [(a, 8), (b, 4), (c, 4), (d, 4), (e, 12)]
        .iter()
        .all(|(group, len)| {
            group.len() == *len && group.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        && c.starts_with('4')
        && d.starts_with(['8', '9', 'a', 'b'])
}

/// One row of the table: a configuration change, the update it must report, and what
/// the cluster must report once the clock completes that update.
struct Row {
    case: &'static str,
    cluster: &'static str,
    body: serde_json::Value,
    /// The `UpdateType` the change reports.
    update_type: &'static str,
    /// The `UpdateParamType` keys it reports, sorted.
    param_types: Vec<&'static str>,
    /// JSON pointers into the `Cluster` and the values they must carry once the
    /// update has completed. The FIRST pair is also the one checked WHILE the update
    /// is in flight, where it must still differ.
    expected: Vec<(&'static str, serde_json::Value)>,
}

/// The table: one row per category of configuration `UpdateClusterConfig` can change,
/// plus a row that changes two categories at once. The values are modelled members —
/// the log types are `LogType`'s enum values, the tier is one of
/// `ProvisionedControlPlaneTier`'s (`standard | tier-xl | tier-2xl | tier-4xl |
/// tier-8xl`), the authentication mode is one of `AuthenticationMode`'s and the egress
/// mode one of `ControlPlaneEgressModeType`'s, none of which the issue cited.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "exporting two control plane log types",
            cluster: "cfg-logging",
            body: serde_json::json!({
                "logging": { "clusterLogging": [{ "types": ["api", "audit"], "enabled": true }] },
            }),
            update_type: "LoggingUpdate",
            param_types: vec!["ClusterLogging"],
            expected: vec![(
                "/logging/clusterLogging/0/types",
                serde_json::json!(["api", "audit"]),
            )],
        },
        Row {
            case: "all three endpoint access members at once",
            cluster: "cfg-endpoint-access",
            body: serde_json::json!({
                "resourcesVpcConfig": {
                    "endpointPublicAccess": false,
                    "endpointPrivateAccess": true,
                    "publicAccessCidrs": ["10.0.0.0/8"],
                },
            }),
            update_type: "EndpointAccessUpdate",
            param_types: vec![
                "EndpointPrivateAccess",
                "EndpointPublicAccess",
                "PublicAccessCidrs",
            ],
            expected: vec![
                (
                    "/resourcesVpcConfig/endpointPrivateAccess",
                    serde_json::json!(true),
                ),
                (
                    "/resourcesVpcConfig/endpointPublicAccess",
                    serde_json::json!(false),
                ),
                (
                    "/resourcesVpcConfig/publicAccessCidrs",
                    serde_json::json!(["10.0.0.0/8"]),
                ),
                // The members the request did not carry are untouched.
                (
                    "/resourcesVpcConfig/subnetIds",
                    serde_json::json!(["subnet-aaaa1111", "subnet-bbbb2222"]),
                ),
            ],
        },
        Row {
            case: "different subnets and security groups",
            cluster: "cfg-vpc-resources",
            body: serde_json::json!({
                "resourcesVpcConfig": {
                    "subnetIds": ["subnet-cccc3333", "subnet-dddd4444", "subnet-eeee5555"],
                    "securityGroupIds": ["sg-11111111", "sg-22222222"],
                },
            }),
            update_type: "VpcConfigUpdate",
            param_types: vec!["SecurityGroups", "Subnets"],
            expected: vec![
                (
                    "/resourcesVpcConfig/subnetIds",
                    serde_json::json!(["subnet-cccc3333", "subnet-dddd4444", "subnet-eeee5555"]),
                ),
                (
                    "/resourcesVpcConfig/securityGroupIds",
                    serde_json::json!(["sg-11111111", "sg-22222222"]),
                ),
            ],
        },
        Row {
            case: "the control plane egress mode",
            cluster: "cfg-egress-mode",
            body: serde_json::json!({
                "resourcesVpcConfig": { "controlPlaneEgressMode": "CUSTOMER_ROUTED" },
            }),
            update_type: "ControlPlaneEgressUpdate",
            param_types: vec!["ControlPlaneEgressMode"],
            expected: vec![(
                "/resourcesVpcConfig/controlPlaneEgressMode",
                serde_json::json!("CUSTOMER_ROUTED"),
            )],
        },
        Row {
            case: "the authentication mode, one step along the ladder",
            cluster: "cfg-access-config",
            body: serde_json::json!({
                "accessConfig": { "authenticationMode": "API_AND_CONFIG_MAP" },
            }),
            update_type: "AccessConfigUpdate",
            param_types: vec!["AuthenticationMode"],
            expected: vec![(
                "/accessConfig/authenticationMode",
                serde_json::json!("API_AND_CONFIG_MAP"),
            )],
        },
        Row {
            case: "leaving extended support",
            cluster: "cfg-upgrade-policy",
            body: serde_json::json!({ "upgradePolicy": { "supportType": "STANDARD" } }),
            update_type: "UpgradePolicyUpdate",
            param_types: vec!["UpgradePolicy"],
            expected: vec![("/upgradePolicy/supportType", serde_json::json!("STANDARD"))],
        },
        Row {
            case: "enabling ARC zonal shift",
            cluster: "cfg-zonal-shift",
            body: serde_json::json!({ "zonalShiftConfig": { "enabled": true } }),
            update_type: "ZonalShiftConfigUpdate",
            param_types: vec!["ZonalShiftConfig"],
            expected: vec![("/zonalShiftConfig/enabled", serde_json::json!(true))],
        },
        Row {
            case: "all three EKS Auto Mode capabilities at once",
            cluster: "cfg-auto-mode",
            body: serde_json::json!({
                "computeConfig": {
                    "enabled": true,
                    "nodePools": ["general-purpose"],
                    "nodeRoleArn": "arn:aws:iam::000000000000:role/eksNodeRole",
                },
                "storageConfig": { "blockStorage": { "enabled": true } },
                "kubernetesNetworkConfig": { "elasticLoadBalancing": { "enabled": true } },
            }),
            update_type: "AutoModeUpdate",
            param_types: vec!["ComputeConfig", "KubernetesNetworkConfig", "StorageConfig"],
            expected: vec![
                ("/computeConfig/enabled", serde_json::json!(true)),
                (
                    "/computeConfig/nodePools",
                    serde_json::json!(["general-purpose"]),
                ),
                (
                    "/storageConfig/blockStorage/enabled",
                    serde_json::json!(true),
                ),
                (
                    "/kubernetesNetworkConfig/elasticLoadBalancing/enabled",
                    serde_json::json!(true),
                ),
                // The network members that can only be set at creation keep what the
                // create wrote.
                (
                    "/kubernetesNetworkConfig/ipFamily",
                    serde_json::json!("ipv4"),
                ),
            ],
        },
        Row {
            case: "adding hybrid node networks",
            cluster: "cfg-remote-network",
            body: serde_json::json!({
                "remoteNetworkConfig": {
                    "remoteNodeNetworks": [{ "cidrs": ["10.2.0.0/16"] }],
                    "remotePodNetworks": [{ "cidrs": ["10.3.0.0/16"] }],
                },
            }),
            update_type: "RemoteNetworkConfigUpdate",
            param_types: vec!["RemoteNetworkConfig"],
            expected: vec![
                (
                    "/remoteNetworkConfig/remoteNodeNetworks/0/cidrs",
                    serde_json::json!(["10.2.0.0/16"]),
                ),
                (
                    "/remoteNetworkConfig/remotePodNetworks/0/cidrs",
                    serde_json::json!(["10.3.0.0/16"]),
                ),
            ],
        },
        Row {
            case: "enabling deletion protection",
            cluster: "cfg-deletion-protection",
            body: serde_json::json!({ "deletionProtection": true }),
            update_type: "DeletionProtectionUpdate",
            param_types: vec!["DeletionProtection"],
            expected: vec![("/deletionProtection", serde_json::json!(true))],
        },
        Row {
            case: "a bigger provisioned control plane tier",
            cluster: "cfg-scaling-tier",
            body: serde_json::json!({ "controlPlaneScalingConfig": { "tier": "tier-2xl" } }),
            update_type: "ControlPlaneScalingConfigUpdate",
            // A tier change reports the tier it moves TO beside the one it moves
            // FROM, which a cluster that asked for no provisioned control plane
            // reports as the `standard` tier it runs on.
            param_types: vec!["PreviousTier", "UpdatedTier"],
            expected: vec![(
                "/controlPlaneScalingConfig/tier",
                serde_json::json!("tier-2xl"),
            )],
        },
        Row {
            case: "two control plane component configurations",
            cluster: "cfg-components",
            body: serde_json::json!({
                "kubeApiServerConfig": {
                    "eventTtl": "2h",
                    "serviceNodePortRange": { "minPort": 30100, "maxPort": 32100 },
                },
                "kubeControllerManagerConfig": {
                    "podGcControllerConfig": { "terminatedPodGcThreshold": 500 },
                },
            }),
            update_type: "ControlPlaneComponentConfigUpdate",
            param_types: vec!["KubeApiServerConfig", "KubeControllerManagerConfig"],
            expected: vec![
                ("/kubeApiServerConfig/eventTtl", serde_json::json!("2h")),
                (
                    "/kubeApiServerConfig/serviceNodePortRange/minPort",
                    serde_json::json!(30100),
                ),
                (
                    "/kubeControllerManagerConfig/podGcControllerConfig/terminatedPodGcThreshold",
                    serde_json::json!(500),
                ),
            ],
        },
        Row {
            case: "two categories in one request",
            cluster: "cfg-two-categories",
            body: serde_json::json!({
                "logging": {
                    "clusterLogging": [{ "types": ["authenticator"], "enabled": true }],
                },
                "deletionProtection": true,
            }),
            // One `Update` carries one `type`, so a change spanning two categories
            // reports the generic configuration update.
            update_type: "ConfigUpdate",
            param_types: vec!["ClusterLogging", "DeletionProtection"],
            expected: vec![
                (
                    "/logging/clusterLogging/0/types",
                    serde_json::json!(["authenticator"]),
                ),
                ("/deletionProtection", serde_json::json!(true)),
            ],
        },
    ]
}

// ---------------------------------------------------------------------------
// RULE: a configuration change starts one asynchronous update, typed by the category
// of configuration it changes and carrying one param per member it asked for; the
// cluster keeps its old configuration and reports UPDATING until the clock completes
// the update, and only then reports the requested values and ACTIVE.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_config_change_starts_a_typed_update_that_completes_onto_the_cluster() {
    let router = emulator();
    let rows = rows();

    for row in &rows {
        let created = create(&router, row.cluster).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200 from CreateCluster, got {created:?}",
            row.case
        );
    }
    settle_creation().await;

    let mut ids = BTreeSet::new();
    for row in &rows {
        let before = describe(&router, row.cluster).await;
        let (pointer, expected) = &row.expected[0];
        let was = before.cluster().pointer(pointer).cloned();

        let started = update_config(&router, row.cluster, &row.body).await;
        assert_eq!(
            started.status, 200,
            "case {:?}: expected 200 from UpdateClusterConfig, got {started:?}",
            row.case
        );

        let id = started.update_id();
        assert!(
            is_uuid_v4(&id),
            "case {:?}: expected a v4 UUID update id, got {id:?}",
            row.case
        );
        assert!(
            ids.insert(id.clone()),
            "case {:?}: update id {id:?} was already minted for another cluster",
            row.case
        );
        assert_eq!(
            started.update()["status"],
            serde_json::json!("InProgress"),
            "case {:?}: a freshly started update is InProgress, got {:?}",
            row.case,
            started.update()["status"]
        );
        assert_eq!(
            started.update()["type"],
            serde_json::json!(row.update_type),
            "case {:?}: expected update type {:?}, got {:?}",
            row.case,
            row.update_type,
            started.update()["type"]
        );
        assert_eq!(
            started.param_types(),
            row.param_types,
            "case {:?}: expected exactly these update params, got {:?}",
            row.case,
            started.update()["params"]
        );
        assert!(
            started.update()["createdAt"].is_number(),
            "case {:?}: expected an epoch-seconds createdAt, got {:?}",
            row.case,
            started.update()["createdAt"]
        );

        // The update is asynchronous: while it runs the cluster is UPDATING and still
        // reports the configuration it had, and the update reads back through
        // DescribeUpdate in that transient state.
        let during = describe(&router, row.cluster).await;
        assert_eq!(
            during.cluster()["status"],
            serde_json::json!("UPDATING"),
            "case {:?}: a cluster under an update is UPDATING, got {:?}",
            row.case,
            during.cluster()["status"]
        );
        let still = during.cluster().pointer(pointer).cloned();
        assert_eq!(
            still, was,
            "case {:?}: {pointer} must not change until the update completes, got {still:?}",
            row.case
        );
        assert_ne!(
            still.as_ref(),
            Some(expected),
            "case {:?}: {pointer} already reports the requested value while the update is \
             in flight",
            row.case
        );

        let polled = describe_update(&router, row.cluster, &id).await;
        assert_eq!(
            polled.update(),
            started.update(),
            "case {:?}: DescribeUpdate must report the update that was started, got {polled:?}",
            row.case
        );
    }

    settle_update().await;

    for row in &rows {
        let after = describe(&router, row.cluster).await;
        assert_eq!(
            after.cluster()["status"],
            serde_json::json!("ACTIVE"),
            "case {:?}: a completed update hands the cluster back to ACTIVE, got {:?}",
            row.case,
            after.cluster()["status"]
        );
        for (pointer, expected) in &row.expected {
            let actual = after.cluster().pointer(pointer).cloned();
            assert_eq!(
                actual.as_ref(),
                Some(expected),
                "case {:?}: expected {pointer} to be {expected:?} once the update completed, \
                 got {actual:?}",
                row.case
            );
        }
    }

    assert_eq!(
        ids.len(),
        rows.len(),
        "every change must mint its own update id, got {ids:?}"
    );
}

/// The same rule, read through the update rather than the cluster: the params carry
/// the values the request asked for, and the update settles to `Successful`.
#[tokio::test]
async fn the_update_reports_the_requested_values_and_settles_to_successful() {
    let router = emulator();

    let cases: Vec<(&str, &str, serde_json::Value, &str, serde_json::Value)> = vec![
        (
            "a logging configuration",
            "param-logging",
            serde_json::json!({
                "logging": { "clusterLogging": [{ "types": ["scheduler"], "enabled": false }] },
            }),
            "ClusterLogging",
            serde_json::json!({
                "clusterLogging": [{ "types": ["scheduler"], "enabled": false }],
            }),
        ),
        (
            "a public access CIDR list",
            "param-cidrs",
            serde_json::json!({
                "resourcesVpcConfig": { "publicAccessCidrs": ["192.168.0.0/16", "10.1.0.0/16"] },
            }),
            "PublicAccessCidrs",
            serde_json::json!(["192.168.0.0/16", "10.1.0.0/16"]),
        ),
        (
            "a boolean toggle",
            "param-deletion-protection",
            serde_json::json!({ "deletionProtection": true }),
            "DeletionProtection",
            serde_json::json!(true),
        ),
        (
            "a compute capability",
            "param-compute",
            serde_json::json!({ "computeConfig": { "enabled": false } }),
            "ComputeConfig",
            serde_json::json!({ "enabled": false }),
        ),
        (
            "hybrid node networks being removed",
            "param-remote-network",
            serde_json::json!({
                "remoteNetworkConfig": { "remoteNodeNetworks": [], "remotePodNetworks": [] },
            }),
            "RemoteNetworkConfig",
            serde_json::json!({ "remoteNodeNetworks": [], "remotePodNetworks": [] }),
        ),
    ];

    for (case, name, _, _, _) in &cases {
        let created = create(&router, name).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateCluster, got {created:?}"
        );
    }
    settle_creation().await;

    let mut started_ids = Vec::new();
    for (case, name, body, param_type, expected) in &cases {
        let started = update_config(&router, name, body).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected 200 from UpdateClusterConfig, got {started:?}"
        );
        assert_eq!(
            &started.param_value(param_type),
            expected,
            "case {case:?}: expected the {param_type} param to carry the requested value, \
             got {:?}",
            started.update()["params"]
        );
        started_ids.push(started.update_id());
    }

    settle_update().await;

    for ((case, name, _, _, _), id) in cases.iter().zip(&started_ids) {
        let settled = describe_update(&router, name, id).await;
        assert_eq!(
            settled.update()["status"],
            serde_json::json!("Successful"),
            "case {case:?}: the clock owes the update Successful, got {:?}",
            settled.update()["status"]
        );
        let again = describe_update(&router, name, id).await;
        assert_eq!(
            again.update(),
            settled.update(),
            "case {case:?}: reading the update again must not change it, got {again:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a configuration change is refused with the modelled error for WHY it cannot be
// made — 404 for a cluster this region does not hold, 409 while another update of the
// cluster is in flight, 400 `InvalidRequestException` for a cluster whose state cannot
// take an update, and 400 `InvalidParameterException` for a value the configuration
// rules do not allow.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_change_that_cannot_be_made_is_refused_with_its_own_modelled_error() {
    let router = emulator();

    // Three clusters with different starting configurations, so each row can be judged
    // against the state its rule is about.
    let created = create(&router, "refuse-plain").await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateCluster, got {created:?}"
    );
    let created = create_with(
        &router,
        "refuse-api-mode",
        serde_json::json!({ "accessConfig": { "authenticationMode": "API" } }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateCluster, got {created:?}"
    );
    let created = create_with(
        &router,
        "refuse-customer-routed",
        serde_json::json!({
            "resourcesVpcConfig": {
                "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"],
                "controlPlaneEgressMode": "CUSTOMER_ROUTED",
            },
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateCluster, got {created:?}"
    );
    let registered = call(
        &router,
        REGION,
        "POST",
        "/cluster-registrations",
        &serde_json::json!({
            "name": "refuse-connected",
            "connectorConfig": { "roleArn": PASSABLE_ROLE, "provider": "EKS_ANYWHERE" },
        })
        .to_string(),
    )
    .await;
    assert_eq!(
        registered.status, 200,
        "setup: expected 200 from RegisterCluster, got {registered:?}"
    );
    settle_creation().await;

    let cases: Vec<(&str, &str, serde_json::Value, u16, &str)> = vec![
        (
            "a cluster this region does not hold",
            "refuse-absent",
            serde_json::json!({ "deletionProtection": true }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a request that asks for no change at all",
            "refuse-plain",
            serde_json::json!({}),
            400,
            "InvalidParameterException",
        ),
        (
            "a single subnet, where at least two are required",
            "refuse-plain",
            serde_json::json!({ "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111"] } }),
            400,
            "InvalidParameterException",
        ),
        (
            "six security groups, where at most five are allowed",
            "refuse-plain",
            serde_json::json!({
                "resourcesVpcConfig": {
                    "securityGroupIds": [
                        "sg-11111111", "sg-22222222", "sg-33333333",
                        "sg-44444444", "sg-55555555", "sg-66666666",
                    ],
                },
            }),
            400,
            "InvalidParameterException",
        ),
        (
            "an IP family, which only a create can set",
            "refuse-plain",
            serde_json::json!({ "kubernetesNetworkConfig": { "ipFamily": "ipv6" } }),
            400,
            "InvalidParameterException",
        ),
        (
            "a service CIDR, which only a create can set",
            "refuse-plain",
            serde_json::json!({
                "kubernetesNetworkConfig": { "serviceIpv4Cidr": "172.20.0.0/16" },
            }),
            400,
            "InvalidParameterException",
        ),
        (
            "a node role in another account",
            "refuse-plain",
            serde_json::json!({
                "computeConfig": {
                    "enabled": true,
                    "nodeRoleArn": "arn:aws:iam::210987654321:role/eksNodeRole",
                },
            }),
            400,
            "InvalidParameterException",
        ),
        (
            "an authentication mode earlier on the one-way ladder",
            "refuse-api-mode",
            serde_json::json!({ "accessConfig": { "authenticationMode": "CONFIG_MAP" } }),
            400,
            "InvalidParameterException",
        ),
        (
            "an egress mode reverting to AWS_MANAGED",
            "refuse-customer-routed",
            serde_json::json!({
                "resourcesVpcConfig": { "controlPlaneEgressMode": "AWS_MANAGED" },
            }),
            400,
            "InvalidParameterException",
        ),
        (
            "a connected cluster, which EKS does not manage a control plane for",
            "refuse-connected",
            serde_json::json!({ "deletionProtection": true }),
            400,
            "ClientException",
        ),
    ];

    for (case, name, body, status, error_type) in &cases {
        let refused = update_config(&router, name, body).await;
        assert_eq!(
            refused.status, *status,
            "case {case:?}: expected HTTP {status}, got {refused:?}"
        );
        let reported = refused.error_type.clone().unwrap_or_default();
        assert!(
            reported.ends_with(error_type),
            "case {case:?}: expected {error_type}, got {reported:?} ({refused:?})"
        );
        // A refused change writes nothing: the cluster is still the one it was.
        if *status != 404 {
            let cluster = describe(&router, name).await;
            assert_ne!(
                cluster.cluster()["status"],
                serde_json::json!("UPDATING"),
                "case {case:?}: a refused change must not take the cluster into an update, \
                 got {:?}",
                cluster.cluster()["status"]
            );
        }
    }
}

/// A cluster that is not running cannot take an update: the one still being created is
/// refused with the state error, and the same request succeeds once the clock has made
/// it `ACTIVE`.
#[tokio::test]
async fn a_cluster_that_is_not_active_cannot_take_a_change() {
    let router = emulator();
    let created = create(&router, "not-yet-active").await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateCluster, got {created:?}"
    );
    assert_eq!(
        created.cluster()["status"],
        serde_json::json!("CREATING"),
        "setup: a new cluster is CREATING, got {:?}",
        created.cluster()["status"]
    );

    let body = serde_json::json!({ "deletionProtection": true });
    let refused = update_config(&router, "not-yet-active", &body).await;
    assert_eq!(
        refused.status, 400,
        "a cluster that is still CREATING cannot take an update, got {refused:?}"
    );
    let reported = refused.error_type.clone().unwrap_or_default();
    assert!(
        reported.ends_with("InvalidRequestException"),
        "expected InvalidRequestException for a non-ACTIVE cluster, got {reported:?}"
    );

    settle_creation().await;
    let accepted = update_config(&router, "not-yet-active", &body).await;
    assert_eq!(
        accepted.status, 200,
        "the same request succeeds once the cluster is ACTIVE, got {accepted:?}"
    );
}

/// One update at a time per cluster, and a retry of the call that started one is
/// absorbed: the idempotency token is what tells the two apart.
#[tokio::test]
async fn one_update_at_a_time_and_a_retried_call_is_absorbed() {
    let router = emulator();
    let created = create(&router, "one-at-a-time").await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateCluster, got {created:?}"
    );
    settle_creation().await;

    let first = update_config(
        &router,
        "one-at-a-time",
        &serde_json::json!({
            "clientRequestToken": "token-1",
            "logging": { "clusterLogging": [{ "types": ["api"], "enabled": true }] },
        }),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "expected 200 from the first UpdateClusterConfig, got {first:?}"
    );

    // The SDK resends the same `clientRequestToken` when it retries, so the retry is
    // answered with the update that is already running.
    let retried = update_config(
        &router,
        "one-at-a-time",
        &serde_json::json!({
            "clientRequestToken": "token-1",
            "logging": { "clusterLogging": [{ "types": ["api"], "enabled": true }] },
        }),
    )
    .await;
    assert_eq!(
        retried.status, 200,
        "a retry carrying the same token is absorbed, got {retried:?}"
    );
    assert_eq!(
        retried.update_id(),
        first.update_id(),
        "a retry must be answered with the update the first call started, got {retried:?}"
    );

    // A DIFFERENT call, while that update is in flight, finds the cluster in use.
    let second = update_config(
        &router,
        "one-at-a-time",
        &serde_json::json!({
            "clientRequestToken": "token-2",
            "deletionProtection": true,
        }),
    )
    .await;
    assert_eq!(
        second.status, 409,
        "a second update while one is in flight is a conflict, got {second:?}"
    );
    let reported = second.error_type.clone().unwrap_or_default();
    assert!(
        reported.ends_with("ResourceInUseException"),
        "expected ResourceInUseException while an update runs, got {reported:?}"
    );

    // And once the first update has completed, the cluster takes another.
    settle_update().await;
    let third = update_config(
        &router,
        "one-at-a-time",
        &serde_json::json!({ "deletionProtection": true }),
    )
    .await;
    assert_eq!(
        third.status, 200,
        "a completed update frees the cluster for the next one, got {third:?}"
    );
    assert_ne!(
        third.update_id(),
        first.update_id(),
        "the next change mints its own update id, got {third:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: a cluster's update history is BOUNDED at `MAX_UPDATES_PER_CLUSTER` entries —
// this operation mints one record per accepted change, so the cap is enforced as the
// history grows, by evicting the oldest COMPLETED update rather than refusing the
// change.
// ---------------------------------------------------------------------------

/// The precondition this suite needs: a history already AT the cap, every entry
/// completed, written into the same `ClusterRecord::updates` map the handlers read.
/// Returns the seeded ids, oldest first.
fn seed_completed_history(
    engine: &engine::InMemoryEngine,
    cluster: &str,
    count: usize,
) -> Vec<String> {
    let store = engine.state::<ComAmazonawsEksAwswesleyfrontendState>();
    let mut guard = store.lock();
    let record = guard
        .clusters
        .get_mut(&(REGION.to_string(), cluster.to_string()))
        .expect("setup: the cluster was created above");
    // The cluster has taken its creation move, so the seeded history is the only
    // thing the handler finds to materialise.
    record.active_at = None;
    record.status = "ACTIVE".to_string();
    let base = SystemTime::now() - Duration::from_secs(count as u64 + 1);
    (0..count)
        .map(|index| {
            let id = format!("00000000-0000-4000-8000-{index:012}");
            record.updates.insert(
                id.clone(),
                UpdateRecord {
                    id: id.clone(),
                    r#type: "ConfigUpdate".to_string(),
                    status: UPDATE_SUCCESSFUL.to_string(),
                    created_at: base + Duration::from_secs(index as u64),
                    params: Vec::new(),
                    client_request_token: None,
                    succeeds_at: None,
                    pending_config: None,
                    cancellation: None,
                    cancellation_request_token: None,
                    // A configuration update is an update of the cluster itself.
                    scope: UpdateScope::Cluster,
                },
            );
            id
        })
        .collect()
}

/// How many updates the store holds for one cluster.
fn stored_update_count(engine: &engine::InMemoryEngine, cluster: &str) -> usize {
    engine
        .state::<ComAmazonawsEksAwswesleyfrontendState>()
        .lock()
        .clusters
        .get(&(REGION.to_string(), cluster.to_string()))
        .map_or(0, |record| record.updates.len())
}

#[tokio::test]
async fn the_update_history_is_bounded_by_evicting_the_oldest_completed_update() {
    let (router, engine) = emulator_with_engine();
    let cluster = "history-bound";
    let created = create(&router, cluster).await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateCluster, got {created:?}"
    );
    settle_creation().await;

    let seeded = seed_completed_history(&engine, cluster, MAX_UPDATES_PER_CLUSTER);
    assert_eq!(
        stored_update_count(&engine, cluster),
        MAX_UPDATES_PER_CLUSTER,
        "setup: expected a history at the cap, got {}",
        stored_update_count(&engine, cluster)
    );
    let (oldest, newest) = (
        seeded.first().expect("setup: a seeded id").clone(),
        seeded.last().expect("setup: a seeded id").clone(),
    );

    // One more change, which is one more record: the history stays at the cap.
    let started = update_config(
        &router,
        cluster,
        &serde_json::json!({ "deletionProtection": true }),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "a change at the history cap is accepted, got {started:?}"
    );
    let held = stored_update_count(&engine, cluster);
    assert_eq!(
        held, MAX_UPDATES_PER_CLUSTER,
        "the history must stay at {MAX_UPDATES_PER_CLUSTER} entries, got {held}"
    );

    // And it is the OLDEST completed update that made room: the newest entries and the
    // update just started are all still reported.
    let evicted = describe_update(&router, cluster, &oldest).await;
    assert_eq!(
        evicted.status, 404,
        "case {oldest:?}: the oldest completed update is evicted, got {evicted:?}"
    );
    for kept in [&newest, &started.update_id()] {
        let still = describe_update(&router, cluster, kept).await;
        assert_eq!(
            still.status, 200,
            "case {kept:?}: this update must still be reported, got {still:?}"
        );
    }
}
