// SPDX-License-Identifier: CC0-1.0

//! WHICH members of a `Cluster` a caller sees, and when.
//!
//! Two rules are under test, both over the whole cluster payload — the one
//! `CreateCluster` answers with, the one `DescribeCluster` reads back and the one
//! `DeleteCluster` answers with:
//!
//! * THE PROVISIONING-DERIVED MEMBERS. The API server `endpoint`, the `identity`
//!   the control plane issues tokens under, the `certificateAuthority` it signs
//!   with and the cluster security group Amazon EKS creates for it exist only
//!   once that control plane does, so a cluster still `CREATING` reports none of
//!   them and a cluster that has reached `ACTIVE` reports them from then on. The
//!   idempotency token of the CALL is a member of no cluster, and the
//!   creation-time access flag is reported by the call that created the cluster
//!   and by no read of it.
//! * THE CONFIGURATION MEMBERS. The configuration Amazon EKS resolves for a
//!   cluster is always reported — control plane logging, the security groups and
//!   egress mode of its VPC configuration, the load balancing capability, the
//!   scaling tier and the three Kubernetes component configurations — with the
//!   service's own default wherever the request named nothing, and with what the
//!   request named wherever it did.
//!
//! Every suite runs over a TABLE whose rows differ along what the rules
//! generalise over: how far the cluster's provisioning has got, and which
//! configuration members a request names (none, one, a partial object, every
//! member of an enum). The values the catalog publishes are not restated as
//! literals twice: the last suite reads the defaults a cluster reports back out
//! of `DescribeClusterVersions`, which publishes the same defaults.

use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    CLUSTER_ACTIVATION_DELAY, UPDATE_COMPLETION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{create, delete, describe, minimal_request, REGION};
use crate::wire::{call, emulator, encode_label, Answer};

/// Every member of `LogType`, in the order the model declares them:
/// `smithy_query(query="shape", shape="LogType")` gives
/// `api | audit | authenticator | controllerManager | scheduler`.
const LOG_TYPES: [&str; 5] = [
    "api",
    "audit",
    "authenticator",
    "controllerManager",
    "scheduler",
];

/// Wait out a clock-owed move, with enough slack that a loaded machine does not
/// race the due instant.
async fn settle() {
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;
}

/// A request with `members` merged into the smallest accepted body.
fn request(name: &str, members: serde_json::Value) -> serde_json::Value {
    let mut body = minimal_request(name);
    let (Some(body_members), Some(extra)) = (body.as_object_mut(), members.as_object()) else {
        panic!("both the request and the members merged into it must be objects");
    };
    for (key, value) in extra {
        body_members.insert(key.clone(), value.clone());
    }
    body
}

/// The `logging` a request asks for, as one `LogSetup` entry.
fn logging(types: &[&str], enabled: bool) -> serde_json::Value {
    serde_json::json!({ "clusterLogging": [{ "types": types, "enabled": enabled }] })
}

/// The `clusterLogging` a cluster reports: the enabled group first, then the
/// disabled one, each group present only when it has a type in it.
fn reported_logging(enabled: &[&str]) -> serde_json::Value {
    let disabled: Vec<&str> = LOG_TYPES
        .iter()
        .copied()
        .filter(|log_type| !enabled.contains(log_type))
        .collect();
    let mut groups = Vec::new();
    if !enabled.is_empty() {
        groups.push(serde_json::json!({ "types": enabled, "enabled": true }));
    }
    if !disabled.is_empty() {
        groups.push(serde_json::json!({ "types": disabled, "enabled": false }));
    }
    serde_json::json!({ "clusterLogging": groups })
}

/// The members that only exist once the control plane does, as `(pointer, what
/// a provisioned cluster reports there)`.
const PROVISIONED_MEMBERS: [&str; 4] = [
    "/endpoint",
    "/identity/oidc/issuer",
    "/certificateAuthority/data",
    "/certificateAuthority/active/id",
];

/// Assert that `cluster` reports none of the provisioning-derived members.
fn assert_unprovisioned(case: &str, cluster: &serde_json::Value) {
    for pointer in PROVISIONED_MEMBERS {
        let held = cluster.pointer(pointer).cloned();
        assert!(
            held.is_none() || held == Some(serde_json::Value::Null),
            "case {case:?}: a cluster whose control plane is not provisioned yet \
             reports no {pointer}, got {held:?}"
        );
    }
    let group = cluster.pointer("/resourcesVpcConfig/clusterSecurityGroupId").cloned();
    assert!(
        group.is_none() || group == Some(serde_json::Value::Null),
        "case {case:?}: the cluster security group is created with the control \
         plane, so there is none to report yet, got {group:?}"
    );
}

/// Assert that `cluster` reports every provisioning-derived member.
fn assert_provisioned(case: &str, cluster: &serde_json::Value) {
    for pointer in PROVISIONED_MEMBERS {
        let held = cluster.pointer(pointer).cloned();
        assert!(
            held.as_ref().and_then(serde_json::Value::as_str).is_some_and(|text| !text.is_empty()),
            "case {case:?}: a cluster with a provisioned control plane reports \
             {pointer}, got {held:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the members that describe a provisioned control plane are reported from
// the moment there IS one, and not before; the idempotency token of the call is
// reported by nobody, and the creation-time access flag only by the call that
// created the cluster.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_members_a_provisioned_control_plane_has_appear_when_it_does() {
    let router = emulator();

    // Rows differ along what the request asked for — nothing, an idempotency
    // token, a second IP family, a VPC configuration of its own — because none of
    // it changes WHEN these members appear.
    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("only the required members", minimal_request("prov-minimal")),
        (
            "an idempotency token",
            request(
                "prov-token",
                serde_json::json!({ "clientRequestToken": "4f1a6d5e-6f0b-4f7a-9c21-4a1d2e3f4b5c" }),
            ),
        ),
        (
            "an ipv6 cluster",
            request(
                "prov-ipv6",
                serde_json::json!({ "kubernetesNetworkConfig": { "ipFamily": "ipv6" } }),
            ),
        ),
        (
            "a VPC configuration of its own",
            request(
                "prov-vpc",
                serde_json::json!({
                    "resourcesVpcConfig": {
                        "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"],
                        "securityGroupIds": ["sg-11111111"],
                        "endpointPublicAccess": false,
                        "endpointPrivateAccess": true,
                    },
                }),
            ),
        ),
    ];

    for (case, body) in &cases {
        let created = create(&router, body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateCluster, got {created:?}"
        );
        assert_eq!(
            created.cluster()["status"],
            serde_json::json!("CREATING"),
            "case {case:?}: a new cluster is CREATING, got {:?}",
            created.cluster()["status"]
        );
        assert_unprovisioned(case, created.cluster());
        assert!(
            created.cluster()["clientRequestToken"].is_null(),
            "case {case:?}: the idempotency token of the call is no member of the \
             cluster, got {:?}",
            created.cluster()["clientRequestToken"]
        );
        assert_eq!(
            created.cluster()["accessConfig"]["bootstrapClusterCreatorAdminPermissions"],
            serde_json::json!(true),
            "case {case:?}: the create reports the access flag it decided, got {:?}",
            created.cluster()["accessConfig"]
        );

        // A read while the cluster is still CREATING reports exactly as little.
        let described = describe(&router, cluster_name(body)).await;
        assert_unprovisioned(case, described.cluster());
        assert!(
            described.cluster()["accessConfig"]["bootstrapClusterCreatorAdminPermissions"]
                .is_null(),
            "case {case:?}: a read of the cluster reports no creation-time access \
             flag, got {:?}",
            described.cluster()["accessConfig"]
        );
    }

    settle().await;

    for (case, body) in &cases {
        let described = describe(&router, cluster_name(body)).await;
        assert_eq!(
            described.cluster()["status"],
            serde_json::json!("ACTIVE"),
            "case {case:?}: the clock owes a new cluster ACTIVE, got {:?}",
            described.cluster()["status"]
        );
        assert_provisioned(case, described.cluster());
        assert!(
            described.cluster()["clientRequestToken"].is_null(),
            "case {case:?}: an ACTIVE cluster reports no clientRequestToken either, \
             got {:?}",
            described.cluster()["clientRequestToken"]
        );
    }
}

/// The name a request names its cluster with.
fn cluster_name(body: &serde_json::Value) -> &str {
    body["name"].as_str().expect("the request names a cluster")
}

// ---------------------------------------------------------------------------
// RULE: `DeleteCluster` answers with the same cluster record — so it reports the
// provisioning-derived members exactly when a read of that cluster would, and
// never the creation-time access flag.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_delete_answers_with_the_cluster_as_it_stood() {
    let router = emulator();

    // The one thing that changes what the delete may report: whether the control
    // plane had been provisioned by the time the cluster was deleted.
    let cases: Vec<(&str, &str, bool)> = vec![
        ("deleted while still CREATING", "del-creating", false),
        ("deleted once ACTIVE", "del-active", true),
    ];

    for (case, name, wait) in &cases {
        let created = create(&router, &minimal_request(name)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateCluster, got {created:?}"
        );
        if *wait {
            settle().await;
        }

        // What the cluster reported right before the delete, so the delete's own
        // answer can be compared with it.
        let read_before = describe(&router, name).await;
        assert_eq!(
            read_before.status, 200,
            "case {case:?}: the cluster must read back before the delete, got \
             {read_before:?}"
        );

        let deleted = delete(&router, name).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200 from DeleteCluster, got {deleted:?}"
        );
        assert_eq!(
            deleted.cluster()["status"],
            serde_json::json!("DELETING"),
            "case {case:?}: a deleted cluster is DELETING, got {:?}",
            deleted.cluster()["status"]
        );
        if *wait {
            assert_provisioned(case, deleted.cluster());
        } else {
            assert_unprovisioned(case, deleted.cluster());
        }
        assert!(
            deleted.cluster()["accessConfig"]["bootstrapClusterCreatorAdminPermissions"]
                .is_null(),
            "case {case:?}: the delete reports no creation-time access flag, got {:?}",
            deleted.cluster()["accessConfig"]
        );
        assert!(
            deleted.cluster()["clientRequestToken"].is_null(),
            "case {case:?}: the delete reports no clientRequestToken, got {:?}",
            deleted.cluster()["clientRequestToken"]
        );
        // The configuration members are on the delete's answer too: a client
        // diffing the record it had against the one the delete reports must find
        // the configuration in both.
        for pointer in [
            "/logging/clusterLogging",
            "/resourcesVpcConfig/securityGroupIds",
            "/resourcesVpcConfig/controlPlaneEgressMode",
            "/kubernetesNetworkConfig/elasticLoadBalancing/enabled",
            "/controlPlaneScalingConfig/tier",
            "/accessConfig/authenticationMode",
        ] {
            assert!(
                deleted.cluster().pointer(pointer).is_some(),
                "case {case:?}: the delete reports {pointer}, got {:?}",
                deleted.cluster()
            );
        }
        // Except the configuration of the control plane COMPONENTS: they describe
        // what a running control plane serves with, and this one is being taken
        // down.
        for member in [
            "kubeApiServerConfig",
            "kubeSchedulerConfig",
            "kubeControllerManagerConfig",
        ] {
            assert!(
                deleted.cluster()[member].is_null(),
                "case {case:?}: the delete reports no {member}, got {:?}",
                deleted.cluster()[member]
            );
            // The cluster had it right up to the delete: it is the delete's own
            // answer that leaves it out, not the record that lost it.
            assert!(
                read_before.cluster()[member].is_object(),
                "case {case:?}: the cluster reported {member} before the delete, \
                 got {:?}",
                read_before.cluster()[member]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: the configuration Amazon EKS resolves for a cluster is always reported —
// what the request named where it named something, the service default where it
// named nothing — by the create and by every later read.
// ---------------------------------------------------------------------------

/// One row: the request, and what the cluster must report at each pointer.
struct ConfigurationRow {
    case: &'static str,
    cluster: &'static str,
    body: serde_json::Value,
    expected: Vec<(&'static str, serde_json::Value)>,
}

fn configuration_rows() -> Vec<ConfigurationRow> {
    // Sources for the values a row names: `LogType` (see [`LOG_TYPES`]),
    // `ControlPlaneEgressModeType` (`smithy_query(query="shape",
    // shape="ControlPlaneEgressModeType")`: `AWS_MANAGED | CUSTOMER_ROUTED |
    // CUSTOMER_ISOLATED`), `ProvisionedControlPlaneTier` (`standard | tier-xl |
    // tier-2xl | tier-4xl | tier-8xl`) and `ScoringStrategyType`
    // (`LeastAllocated | MostAllocated`). Each table below names a member the
    // issue did not cite: `CUSTOMER_ISOLATED`, `tier-8xl`, `MostAllocated`.
    vec![
        ConfigurationRow {
            case: "a request that names no configuration at all",
            cluster: "cfgall-defaults",
            body: serde_json::json!({}),
            expected: vec![
                ("/logging", reported_logging(&[])),
                (
                    "/resourcesVpcConfig/securityGroupIds",
                    serde_json::json!([]),
                ),
                (
                    "/resourcesVpcConfig/controlPlaneEgressMode",
                    serde_json::json!("AWS_MANAGED"),
                ),
                (
                    "/kubernetesNetworkConfig/elasticLoadBalancing",
                    serde_json::json!({ "enabled": false }),
                ),
                (
                    "/controlPlaneScalingConfig",
                    serde_json::json!({ "tier": "standard" }),
                ),
            ],
        },
        ConfigurationRow {
            case: "security groups of its own",
            cluster: "cfgall-security-groups",
            body: serde_json::json!({
                "resourcesVpcConfig": {
                    "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"],
                    "securityGroupIds": ["sg-11111111", "sg-22222222"],
                },
            }),
            expected: vec![(
                "/resourcesVpcConfig/securityGroupIds",
                serde_json::json!(["sg-11111111", "sg-22222222"]),
            )],
        },
        ConfigurationRow {
            case: "an egress mode the cluster routes itself",
            cluster: "cfgall-egress",
            body: serde_json::json!({
                "resourcesVpcConfig": {
                    "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"],
                    "controlPlaneEgressMode": "CUSTOMER_ISOLATED",
                },
            }),
            expected: vec![(
                "/resourcesVpcConfig/controlPlaneEgressMode",
                serde_json::json!("CUSTOMER_ISOLATED"),
            )],
        },
        ConfigurationRow {
            case: "the load balancing capability enabled",
            cluster: "cfgall-load-balancing",
            body: serde_json::json!({
                "kubernetesNetworkConfig": { "elasticLoadBalancing": { "enabled": true } },
            }),
            expected: vec![(
                "/kubernetesNetworkConfig/elasticLoadBalancing",
                serde_json::json!({ "enabled": true }),
            )],
        },
        ConfigurationRow {
            case: "two of the five control plane log types exported",
            cluster: "cfgall-logging-two",
            body: serde_json::json!({ "logging": logging(&["audit", "scheduler"], true) }),
            expected: vec![("/logging", reported_logging(&["audit", "scheduler"]))],
        },
        ConfigurationRow {
            case: "every log type exported",
            cluster: "cfgall-logging-all",
            body: serde_json::json!({ "logging": logging(&LOG_TYPES, true) }),
            expected: vec![("/logging", reported_logging(&LOG_TYPES))],
        },
        ConfigurationRow {
            case: "a logging entry that exports nothing",
            cluster: "cfgall-logging-off",
            body: serde_json::json!({ "logging": logging(&["api"], false) }),
            expected: vec![("/logging", reported_logging(&[]))],
        },
        ConfigurationRow {
            case: "the largest provisioned control plane tier",
            cluster: "cfgall-tier",
            body: serde_json::json!({ "controlPlaneScalingConfig": { "tier": "tier-8xl" } }),
            expected: vec![(
                "/controlPlaneScalingConfig",
                serde_json::json!({ "tier": "tier-8xl" }),
            )],
        },
        ConfigurationRow {
            case: "one parameter of the API server, leaving the other defaulted",
            cluster: "cfgall-api-server",
            body: serde_json::json!({ "kubeApiServerConfig": { "eventTtl": "30m" } }),
            expected: vec![
                ("/kubeApiServerConfig/eventTtl", serde_json::json!("30m")),
                (
                    "/kubeApiServerConfig/serviceNodePortRange",
                    serde_json::json!({ "minPort": 30000, "maxPort": 32767 }),
                ),
            ],
        },
        ConfigurationRow {
            case: "the other scheduler scoring strategy, weighted its own way",
            cluster: "cfgall-scheduler",
            body: serde_json::json!({
                "kubeSchedulerConfig": {
                    "nodeResourcesFit": {
                        "scoringStrategy": {
                            "type": "MostAllocated",
                            "resources": [{ "name": "cpu", "weight": 7 }],
                        },
                    },
                },
            }),
            expected: vec![(
                "/kubeSchedulerConfig/nodeResourcesFit/scoringStrategy",
                serde_json::json!({
                    "type": "MostAllocated",
                    "resources": [{ "name": "cpu", "weight": 7 }],
                }),
            )],
        },
        ConfigurationRow {
            case: "one controller of the controller manager, leaving the other defaulted",
            cluster: "cfgall-controller-manager",
            body: serde_json::json!({
                "kubeControllerManagerConfig": {
                    "podGcControllerConfig": { "terminatedPodGcThreshold": 42 },
                },
            }),
            expected: vec![
                (
                    "/kubeControllerManagerConfig/podGcControllerConfig",
                    serde_json::json!({ "terminatedPodGcThreshold": 42 }),
                ),
                (
                    "/kubeControllerManagerConfig/horizontalPodAutoscalerControllerConfig\
                     /horizontalPodAutoscalerSyncPeriod",
                    serde_json::json!("15s"),
                ),
            ],
        },
    ]
}

#[tokio::test]
async fn the_configuration_a_cluster_runs_with_is_always_reported() {
    let router = emulator();
    let rows = configuration_rows();

    for row in &rows {
        let created = create(&router, &request(row.cluster, row.body.clone())).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200 from CreateCluster, got {created:?}",
            row.case
        );
        for (pointer, expected) in &row.expected {
            let actual = created.cluster().pointer(pointer).cloned();
            assert_eq!(
                actual.as_ref(),
                Some(expected),
                "case {:?}: the create must report {pointer} as {expected:?}, got \
                 {actual:?}",
                row.case
            );
        }

        // THE ROUND TRIP: a read of the cluster reports the same configuration.
        let described = describe(&router, row.cluster).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: expected 200 from DescribeCluster, got {described:?}",
            row.case
        );
        for (pointer, expected) in &row.expected {
            let actual = described.cluster().pointer(pointer).cloned();
            assert_eq!(
                actual.as_ref(),
                Some(expected),
                "case {:?}: the read must report {pointer} as {expected:?}, got \
                 {actual:?}",
                row.case
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: the configuration members are resolved where the configuration is
// WRITTEN, so a change to ONE parameter of a component leaves the others where
// the caller put them — not back at the service default.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_change_to_one_parameter_leaves_the_others_where_the_caller_put_them() {
    let router = emulator();
    let cluster = "cfg-one-parameter";
    // The create customises BOTH parameters of the API server and both controllers
    // of the controller manager, so every row below has something of its own to
    // lose.
    let created = create(
        &router,
        &request(
            cluster,
            serde_json::json!({
                "kubeApiServerConfig": {
                    "eventTtl": "2h",
                    "serviceNodePortRange": { "minPort": 30100, "maxPort": 32100 },
                },
                "kubeControllerManagerConfig": {
                    "podGcControllerConfig": { "terminatedPodGcThreshold": 500 },
                    "horizontalPodAutoscalerControllerConfig": {
                        "horizontalPodAutoscalerSyncPeriod": "30s",
                    },
                },
            }),
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateCluster, got {created:?}"
    );
    settle().await;

    // Rows differ along WHICH parameter the change names, and therefore which one
    // it must leave alone: a duration, a port range, one controller of two.
    let rows: Vec<ConfigurationRow> = vec![
        ConfigurationRow {
            case: "one of the API server's two parameters",
            cluster,
            body: serde_json::json!({ "kubeApiServerConfig": { "eventTtl": "45m" } }),
            expected: vec![
                ("/kubeApiServerConfig/eventTtl", serde_json::json!("45m")),
                (
                    "/kubeApiServerConfig/serviceNodePortRange",
                    serde_json::json!({ "minPort": 30100, "maxPort": 32100 }),
                ),
            ],
        },
        ConfigurationRow {
            case: "the other of them",
            cluster,
            body: serde_json::json!({
                "kubeApiServerConfig": {
                    "serviceNodePortRange": { "minPort": 31000, "maxPort": 31500 },
                },
            }),
            expected: vec![
                ("/kubeApiServerConfig/eventTtl", serde_json::json!("45m")),
                (
                    "/kubeApiServerConfig/serviceNodePortRange",
                    serde_json::json!({ "minPort": 31000, "maxPort": 31500 }),
                ),
            ],
        },
        ConfigurationRow {
            case: "one controller of the controller manager",
            cluster,
            body: serde_json::json!({
                "kubeControllerManagerConfig": {
                    "podGcControllerConfig": { "terminatedPodGcThreshold": 900 },
                },
            }),
            expected: vec![
                (
                    "/kubeControllerManagerConfig/podGcControllerConfig\
                     /terminatedPodGcThreshold",
                    serde_json::json!(900),
                ),
                (
                    "/kubeControllerManagerConfig/horizontalPodAutoscalerControllerConfig\
                     /horizontalPodAutoscalerSyncPeriod",
                    serde_json::json!("30s"),
                ),
            ],
        },
    ];

    for row in &rows {
        let started = call(
            &router,
            REGION,
            "POST",
            &format!("/clusters/{}/update-config", encode_label(cluster)),
            &row.body.to_string(),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {:?}: expected 200 from UpdateClusterConfig, got {started:?}",
            row.case
        );
        tokio::time::sleep(UPDATE_COMPLETION_DELAY + Duration::from_millis(250)).await;

        let described = describe(&router, cluster).await;
        for (pointer, want) in &row.expected {
            let actual = described.cluster().pointer(pointer).cloned();
            assert_eq!(
                actual.as_ref(),
                Some(want),
                "case {:?}: expected {pointer} to be {want:?} after the change, got \
                 {actual:?}",
                row.case
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: the component configuration a cluster that overrode nothing reports is
// the DEFAULT the version catalog publishes for that parameter — one value, read
// two ways, so a caller cannot be told two different things.
// ---------------------------------------------------------------------------

async fn describe_versions(router: &MultiServiceRouter) -> Answer {
    call(router, REGION, "GET", "/cluster-versions?includeAll=true", "").await
}

#[tokio::test]
async fn the_defaults_a_cluster_reports_are_the_ones_the_catalog_publishes() {
    let router = emulator();
    let created = create(&router, &minimal_request("catalog-defaults")).await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateCluster, got {created:?}"
    );

    let catalog = describe_versions(&router).await;
    let rows = catalog.body["clusterVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("setup: expected a published calendar, got {catalog:?}"))
        .clone();
    assert!(
        !rows.is_empty(),
        "setup: the catalog must publish at least one version, got {catalog:?}"
    );

    // Every parameter the catalog publishes a default for, as `(the pointer into
    // the published configuration, the pointer into the cluster)`.
    let parameters: [(&str, &str); 4] = [
        (
            "/controlPlaneComponentConfig/kubeApiServerConfig/eventTtl/defaultValue",
            "/kubeApiServerConfig/eventTtl",
        ),
        (
            "/controlPlaneComponentConfig/kubeApiServerConfig/serviceNodePortRange/defaultValue",
            "/kubeApiServerConfig/serviceNodePortRange",
        ),
        (
            "/controlPlaneComponentConfig/kubeSchedulerConfig/nodeResourcesFit/scoringStrategy\
             /defaultValue",
            "/kubeSchedulerConfig/nodeResourcesFit/scoringStrategy",
        ),
        (
            "/controlPlaneComponentConfig/kubeControllerManagerConfig/podGcControllerConfig\
             /terminatedPodGcThreshold/defaultValue",
            "/kubeControllerManagerConfig/podGcControllerConfig/terminatedPodGcThreshold",
        ),
    ];

    for row in &rows {
        let case = row["clusterVersion"].as_str().unwrap_or_default().to_owned();
        for (published, reported) in parameters {
            let expected = row.pointer(published).cloned();
            assert!(
                expected.is_some(),
                "case {case:?}: the catalog must publish a default at {published}, got {row:?}"
            );
            let actual = created.cluster().pointer(reported).cloned();
            assert_eq!(
                actual, expected,
                "case {case:?}: a cluster that overrode nothing must report the \
                 published default at {reported}, got {actual:?}"
            );
        }
    }

    // And the one parameter whose published default is a duration the catalog
    // reports beside the controller it belongs to.
    let published = "/controlPlaneComponentConfig/kubeControllerManagerConfig\
                     /horizontalPodAutoscalerControllerConfig\
                     /horizontalPodAutoscalerSyncPeriod/defaultValue";
    let expected = rows[0].pointer(published).cloned();
    let actual = created
        .cluster()
        .pointer(
            "/kubeControllerManagerConfig/horizontalPodAutoscalerControllerConfig\
             /horizontalPodAutoscalerSyncPeriod",
        )
        .cloned();
    assert_eq!(
        actual, expected,
        "the horizontal pod autoscaler sync period a cluster reports must be the \
         published default, got {actual:?}"
    );
}
