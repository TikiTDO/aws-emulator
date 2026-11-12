// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the CONVERGENCE of a Slurm-orchestrated HyperPod cluster's
//! nodes: a client that holds the `NodeLogicalId` a `BatchAddClusterNodes` answered
//! follows that node from provisioning to removal through the node listing, and the stale
//! identifier it is left with afterwards is refused.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so what
//! these tests exercise is what a real SDK client gets. The harness is the one
//! `inference_deployment_lifecycle` declares; the cluster, the add and the listing readers
//! are the ones `hyperpod_node_identity_reporting` declares, so both suites drive one
//! cluster shape.
//!
//! The test asserts a RULE over a TABLE of inputs rather than one recorded call: a node
//! tracked by the id an add answered is reported as `Running` by the DEFAULT listing once
//! it is provisioned, is absent from it once `BatchDeleteClusterNodes` removes it, and the
//! `BatchReplaceClusterNodes` that then names only that id is refused as a whole while the
//! cluster still takes fresh nodes.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::hyperpod_node_identity_reporting::{
    add_nodes, create_slurm_cluster, listed_logical_ids, summaries, PAST_SETTLE_DUE,
};
use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator};

/// How SageMaker refuses a `BatchReplaceClusterNodes` none of whose identifiers names a
/// node of the cluster: fixed text with no variable part.
const NONE_EXIST: &str = "All provided nodes do not exist. Please check the input and try \
     again.";

/// RULE: a node tracked by the `NodeLogicalId` an add answered is reported as `Running` by
/// the DEFAULT listing once it is provisioned, is absent from it once a delete removes it,
/// and a replacement naming only that identifier is refused as a whole.
#[tokio::test]
async fn a_node_is_tracked_through_its_lifecycle_by_the_id_the_add_answered() {
    // Rows differ along how many nodes one add asks for, which is the count the delete and
    // the stale replacement then name.
    for count in [1_i64, 2, 3] {
        let case = format!("added={count}");
        let emulator = Emulator::new();
        let cluster = format!("tracked-{count}");
        create_slurm_cluster(&emulator, &cluster, 0).await;
        tokio::time::sleep(PAST_SETTLE_DUE).await;
        let added = add_nodes(&emulator, &cluster, "compute", count).await;
        assert_eq!(
            added.len(),
            usize::try_from(count).expect("a positive node count"),
            "case {case}: the add answers one identifier per node, got {added:?}"
        );

        tokio::time::sleep(PAST_SETTLE_DUE).await;
        let listed = emulator
            .ok("ListClusterNodes", &json!({ "ClusterName": cluster }))
            .await;
        let reported = listed_logical_ids(&case, &listed);
        for logical_id in &added {
            assert!(
                reported.contains(logical_id),
                "case {case}: node {logical_id} must be listed once provisioned, got {listed}"
            );
        }
        for summary in summaries(&listed) {
            assert_eq!(
                str_field(&summary["InstanceStatus"], "Status"),
                "Running",
                "case {case}: a provisioned node is Running, got {summary}"
            );
        }

        let deleted = emulator
            .ok(
                "BatchDeleteClusterNodes",
                &json!({ "ClusterName": cluster, "NodeLogicalIds": added.clone() }),
            )
            .await;
        let gone: BTreeSet<String> = deleted
            .get("SuccessfulNodeLogicalIds")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: expected SuccessfulNodeLogicalIds in {deleted}"))
            .iter()
            .map(|entry| entry.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            gone,
            added.iter().cloned().collect::<BTreeSet<String>>(),
            "case {case}: the delete answers every identifier it removed, got {deleted}"
        );

        let listed = emulator
            .ok("ListClusterNodes", &json!({ "ClusterName": cluster }))
            .await;
        let reported = listed_logical_ids(&case, &listed);
        for logical_id in &added {
            assert!(
                !reported.contains(logical_id),
                "case {case}: node {logical_id} is gone after the delete, got {listed}"
            );
        }

        let (status, refused) = emulator
            .call(
                "BatchReplaceClusterNodes",
                &json!({ "ClusterName": cluster, "NodeLogicalIds": added.clone() }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: replacing only removed nodes is refused, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case}: expected ValidationException, got {refused}"
        );
        assert_eq!(
            message(&refused),
            NONE_EXIST,
            "case {case}: expected the all-missing refusal, got {refused}"
        );

        // The cluster still takes nodes: the stale identifiers refused nothing else.
        let fresh = add_nodes(&emulator, &cluster, "compute", 1).await;
        tokio::time::sleep(PAST_SETTLE_DUE).await;
        let listed = emulator
            .ok("ListClusterNodes", &json!({ "ClusterName": cluster }))
            .await;
        let reported = listed_logical_ids(&case, &listed);
        assert!(
            fresh.iter().all(|logical_id| reported.contains(logical_id)),
            "case {case}: the node added after the refusal must be listed, got {listed}"
        );
    }
}
