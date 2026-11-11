// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker HyperPod's `BatchDeleteClusterNodes`: the nodes a
//! request names are dropped from the cluster, with `ListClusterNodes`,
//! `DescribeClusterNode` and `DescribeCluster` as the readers each deletion is checked
//! against.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so what
//! these tests exercise is what a real SDK client gets. The harness is the one
//! `inference_deployment_lifecycle` declares, plus the cluster-and-node harness
//! `hyperpod_cluster_node_replacement` declares (`in_service_cluster`, `nodes_of`,
//! `string_list` and the per-node failure wordings), not a second copy of either.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call:
//!
//! * a call drops exactly the nodes its identifiers name — whichever identifier kind named
//!   them — and answers each accepted identifier in the position the request listed it;
//! * a deleted node is gone from every reader, and the instance group it belonged to asks
//!   for one instance fewer, so the group's count and its nodes cannot disagree;
//! * an identifier the cluster holds no node for, and one whose node is mid-change, is
//!   reported in the matching failure list with the per-node error code of the identifier
//!   kind that named it, leaving the identifiers beside it in the same batch deleted;
//! * a request naming no node at all, or more identifiers than one call may carry, is
//!   refused, and so is one naming a cluster this account does not hold.

use serde_json::{json, Value};

use crate::hyperpod_cluster_node_replacement::{
    in_service_cluster, nodes_of, string_list, Node, IN_USE, PAST_SETTLE_DUE,
};
use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The one sentence this crate answers for a cluster the account does not hold, read from
/// the crate rather than copied, so the suite pins the SHARED wording every cluster
/// operation answers rather than a second spelling of it.
use service_com_amazonaws_sagemaker_sagemaker::state::cluster_not_found;

/// The per-node failure message for a node that cannot be deleted right now, in the terms
/// both of this operation's failure shapes document their subject in (smithy_query
/// shape=BatchDeleteClusterNodesError, shape=BatchDeleteClusterNodeLogicalIdsError). The
/// in-use wording is the one the sibling replacement suite already names ([`IN_USE`]).
const INVALID_STATUS: &str = "The node is in a state that does not allow deletion. Wait for \
     the node to finish any ongoing changes before retrying.";

/// How SageMaker words the per-node failure of an identifier this cluster holds no node
/// for, whichever identifier kind named it: fixed text with no variable part, since the
/// failure entry it sits in already carries the identifier it is about.
const NOT_FOUND: &str = "Node does not exist in the given cluster.";

/// How many instances each instance group of `cluster` currently holds, as
/// `DescribeCluster` reports it in that group's `CurrentCount`.
async fn current_counts(emulator: &Emulator, cluster: &str) -> Vec<(String, i64)> {
    let described = emulator
        .ok("DescribeCluster", &json!({ "ClusterName": cluster }))
        .await;
    described
        .get("InstanceGroups")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected InstanceGroups in {described}"))
        .iter()
        .map(|group| {
            (
                str_field(group, "InstanceGroupName"),
                group
                    .get("CurrentCount")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| panic!("expected CurrentCount in {group}")),
            )
        })
        .collect()
}

/// One row of the deletion table: the case, how many nodes of each group the cluster is
/// created with, the indices of the nodes named by `NodeIds`, and the indices named by
/// `NodeLogicalIds`.
type DeletionRow<'a> = (&'a str, Vec<(&'a str, &'a str, i64)>, Vec<usize>, Vec<usize>);

/// One row of the per-node failure table: the case, the request, the instance ids and the
/// logical ids the call must delete, and the two failure lists it must report —
/// `Value::Null` where the response carries no list of that kind at all.
type FailureRow = (&'static str, Value, Vec<String>, Vec<String>, Value, Value);

/// RULE: a call drops from the cluster exactly the nodes its identifiers name — an
/// `InstanceId` names the node running on that instance and a `NodeLogicalId` the node
/// itself — answers each accepted identifier in the POSITION the request listed it, and
/// leaves the instance group it belonged to asking for one instance fewer, so a deleted
/// node is gone from every reader and is not provisioned again.
#[tokio::test]
async fn a_call_drops_exactly_the_nodes_its_identifiers_name() {
    // Rows differ along what the rule generalises over: the identifier kind, how many
    // nodes one call names, which group they come from, the order they are named in
    // relative to the order the cluster reports them, and whether the whole group is
    // emptied.
    let rows: Vec<DeletionRow> = vec![
        (
            "one-by-instance-id",
            vec![("pool", "ml.c5.large", 3)],
            vec![0],
            vec![],
        ),
        (
            "one-by-logical-id",
            vec![("pool", "ml.c5.large", 3)],
            vec![],
            vec![1],
        ),
        (
            "several-by-instance-id-out-of-order",
            vec![("pool", "ml.m5.large", 4)],
            vec![2, 0],
            vec![],
        ),
        (
            "both-identifier-kinds-in-one-batch",
            vec![("pool", "ml.m5.large", 4)],
            vec![3],
            vec![0, 2],
        ),
        (
            "across-two-groups",
            vec![("controller", "ml.c5.large", 1), ("workers", "ml.g5.xlarge", 3)],
            vec![0],
            vec![2],
        ),
        (
            "the-whole-group",
            vec![("solo", "ml.t3.medium", 2)],
            vec![],
            vec![0, 1],
        ),
    ];

    for (case, groups, by_instance, by_logical) in rows {
        let emulator = Emulator::new();
        let cluster = format!("delete-nodes-{case}");
        let nodes = in_service_cluster(&emulator, &cluster, &groups).await.1;

        let node_ids: Vec<String> = by_instance
            .iter()
            .map(|at| nodes[*at].instance_id.clone())
            .collect();
        let logical_ids: Vec<String> = by_logical
            .iter()
            .map(|at| nodes[*at].logical_id.clone())
            .collect();
        let mut request = json!({ "ClusterName": cluster.clone() });
        if !node_ids.is_empty() {
            request["NodeIds"] = json!(node_ids.clone());
        }
        if !logical_ids.is_empty() {
            request["NodeLogicalIds"] = json!(logical_ids.clone());
        }

        let answered = emulator.ok("BatchDeleteClusterNodes", &request).await;
        assert_eq!(
            string_list(&answered, "Successful").unwrap_or_default(),
            node_ids,
            "case {case}: the instance ids the request named are the ones deleted, got \
             {answered}"
        );
        assert_eq!(
            string_list(&answered, "SuccessfulNodeLogicalIds").unwrap_or_default(),
            logical_ids,
            "case {case}: the logical ids the request named are the ones deleted, got \
             {answered}"
        );
        assert!(
            answered.get("Failed").is_none() && answered.get("FailedNodeLogicalIds").is_none(),
            "case {case}: every identifier named a node, so no failure is reported, got \
             {answered}"
        );

        // Gone from the listing: the nodes left are exactly the ones the call did not name.
        let deleted: Vec<String> = by_instance
            .iter()
            .chain(by_logical.iter())
            .map(|at| nodes[*at].logical_id.clone())
            .collect();
        let expected: Vec<Node> = nodes
            .iter()
            .filter(|node| !deleted.contains(&node.logical_id))
            .cloned()
            .collect();
        let after = nodes_of(&emulator, &cluster).await;
        assert_eq!(
            after, expected,
            "case {case}: the cluster must hold exactly the nodes the call did not name, \
             got {after:?}"
        );

        // Gone from the per-node reader too: its own identifier no longer resolves.
        for logical_id in &deleted {
            let (status, body) = emulator
                .call(
                    "DescribeClusterNode",
                    &json!({ "ClusterName": cluster.clone(), "NodeLogicalId": logical_id }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case}: a deleted node is not described, got {status} with {body}"
            );
        }

        // The group asks for one instance fewer per node deleted from it, so the count it
        // reports and the nodes it holds agree.
        let counts = current_counts(&emulator, &cluster).await;
        for (group, _, declared) in &groups {
            let held = i64::try_from(
                after
                    .iter()
                    .filter(|node| node.instance_group == *group)
                    .count(),
            )
            .expect("a node count that fits");
            let reported = counts
                .iter()
                .find(|(name, _)| name == group)
                .map(|(_, count)| *count)
                .unwrap_or_else(|| panic!("case {case}: expected group {group} in {counts:?}"));
            assert_eq!(
                reported, held,
                "case {case}: group {group} declared {declared} and must now report the \
                 {held} nodes it holds, got {reported}"
            );
        }
    }
}

/// RULE: a per-node failure is reported for the identifier it belongs to, carrying the
/// error code of the IDENTIFIER KIND that named the node — an instance id's failures are
/// coded for the instance (`InstanceIdNotFound`, `InstanceIdInUse`) and a logical id's for
/// the node (`NodeLogicalIdNotFound`, `NodeIdInUse`, `InvalidNodeStatus`) — and leaves
/// every other identifier of the same batch deleted: a batch operation reports per-entry
/// outcomes instead of failing whole, as long as SOME identifier of it could be deleted.
#[tokio::test]
async fn an_identifier_that_names_no_deletable_node_is_reported_in_the_failure_list() {
    let emulator = Emulator::new();
    let cluster = "delete-nodes-failures";
    let nodes = in_service_cluster(&emulator, cluster, &[("pool", "ml.c5.large", 6)])
        .await
        .1;

    // A node mid-change: its replacement is in flight, so it is not in a settled status
    // and cannot be deleted until the change finishes.
    emulator
        .ok(
            "BatchReplaceClusterNodes",
            &json!({
                "ClusterName": cluster,
                "NodeLogicalIds": [nodes[3].logical_id.clone()],
            }),
        )
        .await;

    // Rows differ along what the rule generalises over: the identifier kind, the shape of
    // the unknown identifier, why the node cannot be deleted, and whether a deletable
    // identifier sits beside it in the same batch.
    let rows: Vec<FailureRow> = vec![
        (
            "unknown-instance-id-beside-a-known-one",
            json!({
                "ClusterName": cluster,
                "NodeIds": ["i-0123456789abcdef0", nodes[0].instance_id.clone()],
            }),
            vec![nodes[0].instance_id.clone()],
            vec![],
            json!([{
                "NodeId": "i-0123456789abcdef0",
                "Code": "InstanceIdNotFound",
                "Message": NOT_FOUND,
            }]),
            Value::Null,
        ),
        (
            "unknown-logical-id-beside-a-known-one",
            json!({
                "ClusterName": cluster,
                "NodeLogicalIds": ["no-such-node", nodes[1].logical_id.clone()],
            }),
            vec![],
            vec![nodes[1].logical_id.clone()],
            Value::Null,
            json!([{
                "NodeLogicalId": "no-such-node",
                "Code": "NodeLogicalIdNotFound",
                "Message": NOT_FOUND,
            }]),
        ),
        (
            "mid-change-node-beside-a-settled-one",
            json!({
                "ClusterName": cluster,
                "NodeLogicalIds": [nodes[3].logical_id.clone(), nodes[2].logical_id.clone()],
            }),
            vec![],
            vec![nodes[2].logical_id.clone()],
            Value::Null,
            json!([{
                "NodeLogicalId": nodes[3].logical_id.clone(),
                "Code": "NodeIdInUse",
                "Message": IN_USE,
            }]),
        ),
        (
            // The SAME node as the row above, named by the other identifier kind, beside a
            // deletable one: the condition is one node's, so only the code's vocabulary
            // changes with the identifier that named it.
            "mid-change-node-named-by-its-instance-id",
            json!({
                "ClusterName": cluster,
                "NodeIds": [nodes[3].instance_id.clone(), nodes[4].instance_id.clone()],
            }),
            vec![nodes[4].instance_id.clone()],
            vec![],
            json!([{
                "NodeId": nodes[3].instance_id.clone(),
                "Code": "InstanceIdInUse",
                "Message": IN_USE,
            }]),
            Value::Null,
        ),
        (
            // A shorter instance id (`^i-[a-f0-9]{8}(?:[a-f0-9]{9})?$` admits 8 or 17 hex
            // characters), so the code does not follow the identifier's length.
            "unknown-short-instance-id-beside-a-known-one",
            json!({
                "ClusterName": cluster,
                "NodeIds": ["i-abcdef01", nodes[5].instance_id.clone()],
            }),
            vec![nodes[5].instance_id.clone()],
            vec![],
            json!([{
                "NodeId": "i-abcdef01",
                "Code": "InstanceIdNotFound",
                "Message": NOT_FOUND,
            }]),
            Value::Null,
        ),
    ];

    for (case, request, successful, successful_logical, failed, failed_logical) in rows {
        let answered = emulator.ok("BatchDeleteClusterNodes", &request).await;
        assert_eq!(
            string_list(&answered, "Successful").unwrap_or_default(),
            successful,
            "case {case:?}: the deletable instance ids are the ones deleted, got {answered}"
        );
        assert_eq!(
            string_list(&answered, "SuccessfulNodeLogicalIds").unwrap_or_default(),
            successful_logical,
            "case {case:?}: the deletable logical ids are the ones deleted, got {answered}"
        );
        assert_eq!(
            answered.get("Failed").unwrap_or(&Value::Null),
            &failed,
            "case {case:?}: an instance id that names no deletable node is reported with \
             the instance code for its condition, got {answered}"
        );
        assert_eq!(
            answered.get("FailedNodeLogicalIds").unwrap_or(&Value::Null),
            &failed_logical,
            "case {case:?}: a logical id that names no deletable node is reported with the \
             node code for its condition, got {answered}"
        );
    }

    // The node that was mid-change is still there: a reported failure deleted nothing.
    let after = nodes_of(&emulator, cluster).await;
    assert!(
        after
            .iter()
            .any(|node| node.logical_id == nodes[3].logical_id),
        "case mid-change: a node reported in a failure list is still held, got {after:?}"
    );
    // Once the change it was in finishes, the same node deletes.
    tokio::time::sleep(PAST_SETTLE_DUE).await;
    let answered = emulator
        .ok(
            "BatchDeleteClusterNodes",
            &json!({
                "ClusterName": cluster,
                "NodeLogicalIds": [nodes[3].logical_id.clone()],
            }),
        )
        .await;
    assert_eq!(
        string_list(&answered, "SuccessfulNodeLogicalIds").unwrap_or_default(),
        vec![nodes[3].logical_id.clone()],
        "case settled-again: a node whose change has finished deletes, got {answered}"
    );
    assert!(
        nodes_of(&emulator, cluster).await.is_empty(),
        "case settled-again: every node of the cluster has now been deleted"
    );
    // A node deleted while it reported `Running` is not resurrected by a later read.
    let counts = current_counts(&emulator, cluster).await;
    assert_eq!(
        counts,
        vec![("pool".to_string(), 0)],
        "case settled-again: an emptied group asks for no instances, got {counts:?}"
    );

    // A node that is still COMING UP has not settled either, so it cannot be deleted until
    // it has: the add below leaves the node `Pending` for the span the clock owes it.
    let added = emulator
        .ok(
            "BatchAddClusterNodes",
            &json!({
                "ClusterName": cluster,
                "NodesToAdd": [{ "InstanceGroupName": "pool", "IncrementTargetCountBy": 1 }],
            }),
        )
        .await;
    let provisioning = str_field(
        added
            .get("Successful")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .unwrap_or_else(|| panic!("expected an added node in {added}")),
        "NodeLogicalId",
    );
    let refused = emulator
        .call(
            "BatchDeleteClusterNodes",
            &json!({ "ClusterName": cluster, "NodeLogicalIds": [provisioning.clone()] }),
        )
        .await;
    // Nothing in the batch could be deleted, so the whole request is refused rather than
    // answered with a 200 whose every entry is a failure.
    assert_eq!(
        (refused.0, error_code(&refused.1).as_str()),
        (400, "ValidationException"),
        "case provisioning: a node that has not finished coming up cannot be deleted, got \
         {refused:?}"
    );
    assert_eq!(
        message(&refused.1),
        "All provided nodes failed validation. Please ensure the nodes exist, are not part \
         of the controller group for the Slurm cluster, and are in a removable state.",
        "case provisioning: refusal message, got {refused:?}"
    );
    // And once it has come up, the same identifier deletes — while a node added beside it
    // and still coming up is reported as a per-node failure, because this batch has an
    // identifier it CAN delete.
    tokio::time::sleep(PAST_SETTLE_DUE).await;
    let added_again = emulator
        .ok(
            "BatchAddClusterNodes",
            &json!({
                "ClusterName": cluster,
                "NodesToAdd": [{ "InstanceGroupName": "pool", "IncrementTargetCountBy": 1 }],
            }),
        )
        .await;
    let still_coming_up = str_field(
        added_again
            .get("Successful")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .unwrap_or_else(|| panic!("expected an added node in {added_again}")),
        "NodeLogicalId",
    );
    let answered = emulator
        .ok(
            "BatchDeleteClusterNodes",
            &json!({
                "ClusterName": cluster,
                "NodeLogicalIds": [provisioning.clone(), still_coming_up.clone()],
            }),
        )
        .await;
    assert_eq!(
        string_list(&answered, "SuccessfulNodeLogicalIds").unwrap_or_default(),
        vec![provisioning],
        "case provisioning-settled: a node that has come up deletes, got {answered}"
    );
    assert_eq!(
        answered.get("FailedNodeLogicalIds"),
        Some(&json!([{
            "NodeLogicalId": still_coming_up,
            "Code": "InvalidNodeStatus",
            "Message": INVALID_STATUS,
        }])),
        "case provisioning-beside-a-deletable-node: the node still coming up is reported \
         as a per-node failure, got {answered}"
    );
}

/// RULE: a request that cannot name the nodes to delete is refused outright rather than
/// answered — it must name at least one identifier, and no more than the combined maximum
/// of 50 — and a cluster this account does not hold is the modelled `ResourceNotFound`,
/// whichever form named it.
#[tokio::test]
async fn a_request_that_cannot_name_the_nodes_to_delete_is_refused() {
    let emulator = Emulator::new();
    let cluster = "delete-nodes-refusals";
    let nodes = in_service_cluster(&emulator, cluster, &[("pool", "ml.c5.large", 1)])
        .await
        .1;
    let node = nodes[0].clone();
    // Twenty-six of each kind: inside each list's own modelled bound, past the 50
    // identifiers one call may carry. `ClusterNodeId` is `^i-[a-f0-9]{8}(?:[a-f0-9]{9})?$`,
    // so each id carries 17 hex characters and the refusal under test is the service's own
    // combined bound rather than the modelled pattern.
    let twenty_six: Vec<String> = (0..26)
        .map(|at| format!("i-0123456789abcde{at:02x}"))
        .collect();
    let twenty_six_logical: Vec<String> = (0..26).map(|at| format!("no-such-node-{at}")).collect();
    let missing_arn = format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:cluster/aaaaaaaaaaaa");
    // The case, the request, the modelled error code, and the EXACT message the refusal
    // must carry: a cluster this account does not hold is worded by the one sentence every
    // cluster operation of this crate answers for that fact, echoing the identifier as
    // sent (`ClusterName` is a `ClusterNameOrArn`).
    let rows: Vec<(&str, Value, &str, String)> = vec![
        (
            "no-identifier",
            json!({ "ClusterName": cluster }),
            "ValidationException",
            "Either NodeIds or NodeLogicalIds must be provided.".to_string(),
        ),
        (
            "past-the-batch-bound",
            json!({
                "ClusterName": cluster,
                "NodeIds": twenty_six,
                "NodeLogicalIds": twenty_six_logical,
            }),
            "ValidationException",
            "You can specify up to 50 identifiers in a single request.".to_string(),
        ),
        (
            "unknown-cluster-by-name",
            json!({ "ClusterName": "no-such-cluster", "NodeIds": [node.instance_id.clone()] }),
            "ResourceNotFound",
            cluster_not_found("no-such-cluster"),
        ),
        (
            "unknown-cluster-by-arn",
            json!({
                "ClusterName": missing_arn.clone(),
                "NodeIds": [node.instance_id.clone()],
            }),
            "ResourceNotFound",
            cluster_not_found(&missing_arn),
        ),
    ];
    for (case, request, expected, expected_message) in rows {
        let (status, body) = emulator.call("BatchDeleteClusterNodes", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
        assert_eq!(
            message(&body),
            expected_message,
            "case {case:?}: expected the refusal to be worded {expected_message:?}, got {body}"
        );
    }
    // None of the refusals touched the cluster: the node is still the one it was.
    let after = nodes_of(&emulator, cluster).await;
    assert_eq!(
        after,
        vec![node],
        "case refusals: a refused request deletes nothing, got {after:?}"
    );
}

/// RULE: a node the Slurm workload manager runs on cannot be taken out from under it, so
/// it is never deletable: a batch naming only such nodes is refused as a whole — none of
/// its identifiers could be deleted — while a batch that also names a deletable node
/// deletes that one and reports the controller node in the failure list.
#[tokio::test]
async fn a_slurm_controller_node_cannot_be_deleted() {
    let emulator = Emulator::new();
    let cluster = "slurm-controller-cluster";
    let created = emulator
        .ok(
            "CreateCluster",
            &json!({
                "ClusterName": cluster,
                "Orchestrator": { "Slurm": { "SlurmConfigStrategy": "Managed" } },
                "InstanceGroups": [
                    {
                        "InstanceGroupName": "controller",
                        "InstanceCount": 1,
                        "InstanceType": "ml.m5.large",
                        "ExecutionRole": format!("arn:aws:iam::{ACCOUNT}:role/hyperpod-controller"),
                        // `ClusterSlurmNodeType`'s members are Controller | Login | Compute
                        // (smithy_query shape=ClusterSlurmNodeType).
                        "SlurmConfig": { "NodeType": "Controller" },
                    },
                    {
                        "InstanceGroupName": "compute",
                        "InstanceCount": 2,
                        "InstanceType": "ml.c5.large",
                        "ExecutionRole": format!("arn:aws:iam::{ACCOUNT}:role/hyperpod-compute"),
                        "SlurmConfig": { "NodeType": "Compute" },
                    },
                ],
            }),
        )
        .await;
    assert!(
        !str_field(&created, "ClusterArn").is_empty(),
        "CreateCluster must mint an ARN, got {created:?}"
    );
    tokio::time::sleep(PAST_SETTLE_DUE).await;
    let nodes = nodes_of(&emulator, cluster).await;
    let controller = nodes
        .iter()
        .find(|node| node.instance_group == "controller")
        .cloned()
        .unwrap_or_else(|| panic!("expected a controller node, got {nodes:?}"));
    let compute: Vec<Node> = nodes
        .iter()
        .filter(|node| node.instance_group == "compute")
        .cloned()
        .collect();

    // Rows differ along how the controller node is named — by its instance id and by its
    // logical id. Each batch names ONLY controller nodes, so none of its identifiers could
    // be deleted and the request is refused as a whole.
    let rows: Vec<(&str, Value)> = vec![
        (
            "by instance id",
            json!({ "NodeIds": [controller.instance_id.clone()] }),
        ),
        (
            "by logical id",
            json!({ "NodeLogicalIds": [controller.logical_id.clone()] }),
        ),
    ];
    for (case, extra) in &rows {
        let mut body = json!({ "ClusterName": cluster });
        for (member, value) in extra.as_object().expect("a JSON object of members") {
            body[member] = value.clone();
        }
        let (status, refused) = emulator.call("BatchDeleteClusterNodes", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a controller node must not be deletable, got {status} \
             with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: error code, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            "All provided nodes failed validation. Please ensure the nodes exist, are not \
             part of the controller group for the Slurm cluster, and are in a removable \
             state.",
            "case {case:?}: refusal message, got {refused:?}"
        );
        // Nothing was deleted.
        assert_eq!(
            nodes_of(&emulator, cluster).await,
            nodes,
            "case {case:?}: a refused batch must delete nothing"
        );
    }

    // A batch that also names a deletable node deletes that one and reports the controller
    // node beside it: the per-node failure is what a batch operation answers whenever
    // SOME identifier of it could be deleted.
    let mixed = emulator
        .ok(
            "BatchDeleteClusterNodes",
            &json!({
                "ClusterName": cluster,
                "NodeIds": [compute[0].instance_id.clone(), controller.instance_id.clone()],
            }),
        )
        .await;
    assert_eq!(
        mixed.get("Successful"),
        Some(&json!([compute[0].instance_id.clone()])),
        "case mixed: the deletable node must be deleted, got {mixed:?}"
    );
    assert_eq!(
        mixed.get("Failed"),
        Some(&json!([{
            "NodeId": controller.instance_id.clone(),
            "Code": "InvalidInstanceStatus",
            "Message": "You cannot remove instances that are configured as Slurm controller \
                        nodes.",
        }])),
        "case mixed: the controller node must be reported as a per-node failure, \
         got {mixed:?}"
    );
    assert!(
        nodes_of(&emulator, cluster)
            .await
            .iter()
            .any(|node| node.logical_id == controller.logical_id),
        "case mixed: the controller node must still be held"
    );

    // A replacement of a controller node is refused in that operation's own words.
    let (status, refused) = emulator
        .call(
            "BatchReplaceClusterNodes",
            &json!({ "ClusterName": cluster, "NodeIds": [controller.instance_id.clone()] }),
        )
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case replace: a controller node must not be replaceable, got {status} with {refused}"
    );
    assert_eq!(
        message(&refused),
        "You cannot replace instances that are configured as Slurm controller nodes.",
        "case replace: refusal message, got {refused:?}"
    );

    // The compute node the mixed batch did not delete still deletes, so the refusals above
    // are about the controller node and not about the operation.
    let remaining: Vec<String> = nodes_of(&emulator, cluster)
        .await
        .iter()
        .filter(|node| node.instance_group == "compute")
        .map(|node| node.instance_id.clone())
        .collect();
    let served = emulator
        .ok(
            "BatchDeleteClusterNodes",
            &json!({ "ClusterName": cluster, "NodeIds": remaining.clone() }),
        )
        .await;
    assert_eq!(
        served
            .get("Successful")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(remaining.len()),
        "case compute: the non-controller nodes must delete, got {served:?}"
    );
}

/// RULE: a cluster whose deletion has begun takes no node change, so every node operation
/// against it is refused — naming the cluster and that it is being torn down — and the
/// nodes it still holds are untouched.
#[tokio::test]
async fn a_cluster_being_deleted_takes_no_node_change() {
    let emulator = Emulator::new();
    let cluster = "deleting-cluster";
    let (arn, nodes) = in_service_cluster(&emulator, cluster, &[("pool", "ml.c5.large", 2)]).await;
    let node = nodes[0].clone();

    let deleted = emulator
        .ok("DeleteCluster", &json!({ "ClusterName": cluster }))
        .await;
    assert_eq!(
        str_field(&deleted, "ClusterArn"),
        arn,
        "DeleteCluster must answer the cluster's ARN, got {deleted:?}"
    );
    // The cluster is observably `Deleting` until the clock completes the teardown, so the
    // rows below run against a cluster every reader still reports.
    let described = emulator
        .ok("DescribeCluster", &json!({ "ClusterName": cluster }))
        .await;
    assert_eq!(
        str_field(&described, "ClusterStatus"),
        "Deleting",
        "the cluster must be observably Deleting, got {described:?}"
    );
    let expected_message = format!("Cluster {arn} is in a termination state that cannot be modified.");

    // Rows: every node operation that CHANGES the cluster's nodes, each naming the cluster
    // by name and by ARN.
    let requests: Vec<(&str, &str, Value)> = vec![
        (
            "a delete by instance id",
            "BatchDeleteClusterNodes",
            json!({ "NodeIds": [node.instance_id.clone()] }),
        ),
        (
            "a delete by logical id",
            "BatchDeleteClusterNodes",
            json!({ "NodeLogicalIds": [node.logical_id.clone()] }),
        ),
        (
            "a replacement",
            "BatchReplaceClusterNodes",
            json!({ "NodeIds": [node.instance_id.clone()] }),
        ),
        (
            "an addition",
            "BatchAddClusterNodes",
            json!({
                "NodesToAdd": [{ "InstanceGroupName": "pool", "IncrementTargetCountBy": 1 }],
            }),
        ),
    ];
    for (label, target, extra) in &requests {
        for identifier in [cluster.to_string(), arn.clone()] {
            let case = format!("{label} naming {identifier}");
            let mut body = json!({ "ClusterName": identifier });
            for (member, value) in extra.as_object().expect("a JSON object of members") {
                body[member] = value.clone();
            }
            let (status, refused) = emulator.call(target, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a cluster being deleted must refuse the change, got \
                 {status} with {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {case:?}: error code, got {refused:?}"
            );
            assert_eq!(
                message(&refused),
                expected_message,
                "case {case:?}: the refusal must name the cluster and its status, \
                 got {refused:?}"
            );
        }
    }

    // Nothing changed: the cluster still holds both nodes it held.
    let after = nodes_of(&emulator, cluster).await;
    assert_eq!(
        after, nodes,
        "case deleting: a refused node change must leave every node as it was, got {after:?}"
    );
}
