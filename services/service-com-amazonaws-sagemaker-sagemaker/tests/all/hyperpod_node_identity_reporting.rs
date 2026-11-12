// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the IDENTITY a HyperPod node reader reports: every node a
//! reader answers carries that node's `NodeLogicalId`, the identifier that persists for
//! the node's whole lifecycle, so a client that holds the id one call answered tracks the
//! node through every other call.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so what
//! these tests exercise is what a real SDK client gets. The harness is the one
//! `inference_deployment_lifecycle` declares.
//!
//! The test asserts a RULE over a TABLE of inputs rather than one recorded call:
//! `ListClusterNodes` reports the `NodeLogicalId` of every node it lists — whichever value
//! `IncludeNodeLogicalIds` carries, whichever identifier named the cluster, and whether the
//! node came up with the cluster's own instance groups or was added later — and the ids it
//! reports are the ones `DescribeClusterNode` answers for the same nodes.
//!
//! What a client does with that id over a node's lifecycle is the sibling suite
//! `hyperpod_slurm_node_convergence`, which reads its helpers from here.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{str_field, Emulator, ACCOUNT};

/// Longer than `state::CLUSTER_IN_SERVICE_AFTER` and
/// `state::TRANSIENT_STATE_SETTLES_AFTER` (300 ms each), so the clock owes a created
/// cluster its instances, and an added node its `Pending` -> `Running` transition, by the
/// time the next call runs.
pub(crate) const PAST_SETTLE_DUE: Duration = Duration::from_millis(600);

/// One instance group of a Slurm-orchestrated HyperPod cluster. `InstanceCount`,
/// `InstanceGroupName` and `ExecutionRole` are the group's required members (smithy_query
/// shape=ClusterInstanceGroupSpecification); `SlurmConfig$NodeType` names the role the
/// group plays for the workload manager (smithy_query shape=ClusterSlurmConfig).
fn slurm_group(name: &str, instance_type: &str, count: i64, node_type: &str) -> Value {
    json!({
        "InstanceGroupName": name,
        "InstanceCount": count,
        "InstanceType": instance_type,
        "ExecutionRole": format!("arn:aws:iam::{ACCOUNT}:role/hyperpod-{name}"),
        "SlurmConfig": { "NodeType": node_type },
    })
}

/// Create a Slurm cluster whose nodes are provisioned in the one mode
/// `ClusterNodeProvisioningMode` declares (smithy_query shape=ClusterNodeProvisioningMode),
/// holding a controller group and a compute group of `compute_count` instances. Answers its
/// `ClusterArn`.
pub(crate) async fn create_slurm_cluster(
    emulator: &Emulator,
    name: &str,
    compute_count: i64,
) -> String {
    let created = emulator
        .ok(
            "CreateCluster",
            &json!({
                "ClusterName": name,
                "NodeProvisioningMode": "Continuous",
                "Orchestrator": { "Slurm": {} },
                "InstanceGroups": [
                    slurm_group("controller", "ml.m5.xlarge", 1, "Controller"),
                    slurm_group("compute", "ml.c5.xlarge", compute_count, "Compute"),
                ],
            }),
        )
        .await;
    str_field(&created, "ClusterArn")
}

/// Add `count` nodes to `group` of `cluster`, answering the `NodeLogicalId`s the call
/// minted in the order it answered them.
pub(crate) async fn add_nodes(
    emulator: &Emulator,
    cluster: &str,
    group: &str,
    count: i64,
) -> Vec<String> {
    let added = emulator
        .ok(
            "BatchAddClusterNodes",
            &json!({
                "ClusterName": cluster,
                "NodesToAdd": [{ "InstanceGroupName": group, "IncrementTargetCountBy": count }],
            }),
        )
        .await;
    added
        .get("Successful")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case add {count} to {group}: expected Successful in {added}"))
        .iter()
        .map(|entry| str_field(entry, "NodeLogicalId"))
        .collect()
}

/// The summaries of a `ListClusterNodes` response.
pub(crate) fn summaries(listed: &Value) -> Vec<Value> {
    listed
        .get("ClusterNodeSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected ClusterNodeSummaries in {listed}"))
        .clone()
}

/// The `NodeLogicalId` of every summary a listing carries. A summary that carries none
/// fails the read, which is the rule these tests pin.
pub(crate) fn listed_logical_ids(case: &str, listed: &Value) -> BTreeSet<String> {
    summaries(listed)
        .iter()
        .map(|summary| {
            summary
                .get("NodeLogicalId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case}: every listed node carries its NodeLogicalId, got {summary}"
                    )
                })
                .to_string()
        })
        .collect()
}

/// RULE: every node `ListClusterNodes` reports carries that node's own `NodeLogicalId`,
/// whichever value `IncludeNodeLogicalIds` carries and whichever identifier named the
/// cluster — the flag selects WHICH NODES are reported, not which members a summary
/// carries.
#[tokio::test]
async fn every_listed_node_carries_its_logical_id() {
    /// One listing a client can ask for: how the cluster was named, and the
    /// `IncludeNodeLogicalIds` value the request carried.
    struct Row {
        case: &'static str,
        by_arn: bool,
        flag: Option<bool>,
    }

    // Rows differ along the two axes a listing varies on: the identifier form of the
    // cluster, and each value the flag can take (absent, false, true).
    let rows = [
        Row {
            case: "named by name, flag absent",
            by_arn: false,
            flag: None,
        },
        Row {
            case: "named by name, flag false",
            by_arn: false,
            flag: Some(false),
        },
        Row {
            case: "named by name, flag true",
            by_arn: false,
            flag: Some(true),
        },
        Row {
            case: "named by ARN, flag absent",
            by_arn: true,
            flag: None,
        },
        Row {
            case: "named by ARN, flag true",
            by_arn: true,
            flag: Some(true),
        },
    ];

    for row in rows {
        let emulator = Emulator::new();
        // Two nodes come up with the cluster's own groups and one more is added
        // afterwards, so each case covers both origins a node can have.
        let arn = create_slurm_cluster(&emulator, "identity-cluster", 1).await;
        tokio::time::sleep(PAST_SETTLE_DUE).await;
        let added = add_nodes(&emulator, "identity-cluster", "compute", 1).await;
        tokio::time::sleep(PAST_SETTLE_DUE).await;

        let identifier = if row.by_arn {
            arn.clone()
        } else {
            "identity-cluster".to_string()
        };
        let mut request = json!({ "ClusterName": identifier });
        if let Some(flag) = row.flag {
            request["IncludeNodeLogicalIds"] = Value::Bool(flag);
        }
        let listed = emulator.ok("ListClusterNodes", &request).await;
        let reported = listed_logical_ids(row.case, &listed);

        // Every node of the cluster has an `InstanceId` by now, so every listing reports
        // all three of them whatever the flag says.
        assert_eq!(
            reported.len(),
            3,
            "case {}: expected the cluster's three nodes, got {listed}",
            row.case
        );
        for logical_id in &added {
            assert!(
                reported.contains(logical_id),
                "case {}: the id the add answered ({logical_id}) must identify a listed node, \
                 got {reported:?}",
                row.case
            );
        }
        // The ids a listing reports are the ids the per-node reader answers for the same
        // nodes, so two readers cannot disagree about a node's identity.
        for logical_id in &reported {
            let described = emulator
                .ok(
                    "DescribeClusterNode",
                    &json!({ "ClusterName": identifier, "NodeLogicalId": logical_id }),
                )
                .await;
            let answered = str_field(&described["NodeDetails"], "NodeLogicalId");
            assert_eq!(
                &answered, logical_id,
                "case {}: the per-node reader must answer the same identity, got {described}",
                row.case
            );
        }
    }
}
