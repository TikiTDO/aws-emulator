// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker HyperPod's `BatchRebootClusterNodes`: the nodes a
//! request names are restarted in place, with `DescribeClusterNode` and `ListClusterNodes`
//! as the readers each reboot is checked against.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so what
//! these tests exercise is what a real SDK client gets. The harness is the one
//! `inference_deployment_lifecycle` declares, and the cluster and node helpers are the ones
//! `hyperpod_cluster_node_replacement` declares (`in_service_cluster`, `nodes_of`,
//! `describe`, `string_list`, `Node`).
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call:
//!
//! * a `ClusterName` this account holds no cluster for is refused at the TOP level with the
//!   modelled `ResourceNotFound` and the sentence every cluster-addressing operation words
//!   it with, whichever identifier form named it and whichever identifier kind the request
//!   carried;
//! * every identifier a call accepts is answered in the position the request listed it,
//!   whichever order that is and whichever identifier kind named it;
//! * a rebooted node restarts on the instance it is already running on — the same
//!   `InstanceId` and `LaunchTime` — and reports `Running` again once the clock owes it;
//! * an identifier naming no node of the cluster is reported in the matching failure list
//!   with the modelled per-node error code, leaving the identifiers beside it rebooted;
//! * a node the service is already changing is reported rather than rebooted again;
//! * a request that cannot name the nodes to reboot is refused outright.

use std::time::Instant;

use serde_json::{json, Value};

use crate::hyperpod_cluster_node_replacement::{
    describe, in_service_cluster, nodes_of, string_list, Node, POLL_INTERVAL, SETTLE_DEADLINE,
};
use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The per-node failure messages, as `BatchRebootClusterNodesErrorCode` accounts for each
/// code (smithy_query shape=BatchRebootClusterNodesError,
/// shape=BatchRebootClusterNodeLogicalIdsError): the instance-id list says "instance" and
/// the logical-id list says "node", the terms their own error shapes are documented in.
const INSTANCE_NOT_FOUND: &str = "The instance does not exist in the specified cluster.";
/// See [`INSTANCE_NOT_FOUND`].
const NODE_NOT_FOUND: &str = "The node does not exist in the specified cluster.";
/// See [`INSTANCE_NOT_FOUND`].
const NODE_INVALID_STATUS: &str = "The node is in a state that does not allow rebooting. \
     Wait for the node to finish any ongoing changes before retrying.";
/// See [`INSTANCE_NOT_FOUND`].
const IN_USE: &str = "Another operation is already in progress for this node. Wait for the \
     operation to complete before retrying.";

/// How a batch node operation refuses a request none of whose identifiers names a node of
/// the cluster: fixed text with no variable part.
const NONE_EXIST: &str = "All provided nodes do not exist. Please check the input and try \
     again.";

/// How SageMaker words a cluster this account does not hold, for an operation that
/// addresses the cluster itself: the identifier the request carried, and no trailing
/// period.
fn cluster_not_found(identifier: &str) -> String {
    format!("Cluster with name {identifier} not found")
}

/// The node `logical_id` names, once it reports `Running` again.
///
/// Polled for the CONDITION — the status the clock owes the node back — not for a span of
/// time.
async fn node_once_running(emulator: &Emulator, cluster: &str, logical_id: &str) -> Node {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let found = nodes_of(emulator, cluster)
            .await
            .into_iter()
            .find(|node| node.logical_id == logical_id);
        match found {
            Some(node) if node.status == "Running" => return node,
            other => assert!(
                Instant::now() < deadline,
                "case {logical_id:?}: the rebooted node must report Running again, got {other:?}"
            ),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE: `ClusterName` names the resource a batch node operation acts on, so a name or ARN
/// this account holds no cluster for is refused at the TOP level with the modelled
/// `ResourceNotFound` — worded as every operation addressing the cluster itself words it —
/// and nothing of the request is carried out.
#[tokio::test]
async fn an_unknown_cluster_is_refused_at_the_top_level() {
    let emulator = Emulator::new();
    // Rows differ along the identifier FORM (a short name, a long hyphenated one, a
    // well-formed cluster ARN) and along which identifier kind the request named nodes by.
    let rows: Vec<(&str, String, Value)> = vec![
        (
            "short-name-by-instance-id",
            "A".to_string(),
            json!({ "NodeIds": ["i-0123456789abcdef0"] }),
        ),
        (
            "hyphenated-name-by-instance-id",
            "no-such-hyperpod-cluster-9".to_string(),
            json!({ "NodeIds": ["i-0123456789abcdef0"] }),
        ),
        (
            "name-by-logical-id",
            "missingCluster".to_string(),
            json!({ "NodeLogicalIds": ["some-node"] }),
        ),
        (
            "name-by-both-kinds",
            "absent-cluster".to_string(),
            json!({
                "NodeIds": ["i-abcdef01", "i-0123456789abcdef0"],
                "NodeLogicalIds": ["some-node"],
            }),
        ),
        (
            "arn-form",
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:cluster/aaaaaaaaaaaa"),
            json!({ "NodeIds": ["i-0123456789abcdef0"] }),
        ),
    ];
    for (case, identifier, mut request) in rows {
        request["ClusterName"] = json!(identifier);
        let (status, body) = emulator.call("BatchRebootClusterNodes", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFound",
            "case {case:?}: expected ResourceNotFound, got {body}"
        );
        assert_eq!(
            message(&body),
            cluster_not_found(&identifier),
            "case {case:?}: expected the cluster-not-found sentence, got {body}"
        );
        // Nothing was conjured on the way: the cluster the request named still does not
        // exist, and the refusal is the reader's too.
        let (read_status, read_body) = emulator
            .call("DescribeCluster", &json!({ "ClusterName": identifier }))
            .await;
        assert_eq!(
            read_status, 400,
            "case {case:?}: the refused reboot must create no cluster, got {read_status} with \
             {read_body}"
        );
        assert_eq!(
            error_code(&read_body),
            "ResourceNotFound",
            "case {case:?}: the refused reboot must create no cluster, got {read_body}"
        );
    }
    // The positive control: the cluster listing still answers.
    let listed = emulator
        .ok("ListClusters", &json!({ "MaxResults": 1 }))
        .await;
    assert!(
        listed.get("ClusterSummaries").is_some(),
        "case control: ListClusters still answers, got {listed}"
    );
}

/// RULE: a batch operation answers the identifiers it accepted in the POSITION the request
/// listed them — `Successful` carries the `NodeIds` whose reboot it initiated and
/// `SuccessfulNodeLogicalIds` the `NodeLogicalIds` — so a caller can line its request up
/// against the answer without re-sorting either.
#[tokio::test]
async fn every_accepted_identifier_is_answered_in_the_position_the_request_listed_it() {
    // Rows differ along what the rule generalises over: how many nodes one call names, and
    // in which order relative to the order the cluster reports them.
    let rows: Vec<(&str, Vec<usize>)> = vec![
        ("one", vec![0]),
        ("reported-order", vec![0, 1, 2, 3]),
        ("reversed", vec![3, 2, 1, 0]),
        ("out-of-order-subset", vec![2, 0, 3]),
    ];
    for (case, picks) in rows {
        // By `NodeIds`, and then — on a cluster of its own, so no reboot is already in
        // flight — by `NodeLogicalIds`: the ordering rule is the same for both.
        let emulator = Emulator::new();
        let cluster = format!("reboot-by-instance-id-{case}");
        let nodes = in_service_cluster(&emulator, &cluster, &[("pool", "ml.c5.large", 4)])
            .await
            .1;
        let node_ids: Vec<String> = picks
            .iter()
            .map(|at| nodes[*at].instance_id.clone())
            .collect();
        let rebooted = emulator
            .ok(
                "BatchRebootClusterNodes",
                &json!({ "ClusterName": cluster, "NodeIds": node_ids }),
            )
            .await;
        assert_eq!(
            string_list(&rebooted, "Successful"),
            Some(node_ids.clone()),
            "case {case:?} by instance id: Successful must carry the requested ids in order, \
             got {rebooted}"
        );
        assert_eq!(
            rebooted.get("Failed"),
            Some(&json!([])),
            "case {case:?} by instance id: no requested id failed, got {rebooted}"
        );
        assert_eq!(
            rebooted.get("SuccessfulNodeLogicalIds"),
            None,
            "case {case:?} by instance id: the logical-id lists belong to a request that named \
             logical ids, got {rebooted}"
        );
        assert_eq!(
            rebooted.get("FailedNodeLogicalIds"),
            None,
            "case {case:?} by instance id: the logical-id lists belong to a request that named \
             logical ids, got {rebooted}"
        );

        let emulator = Emulator::new();
        let cluster = format!("reboot-by-logical-id-{case}");
        let nodes = in_service_cluster(&emulator, &cluster, &[("pool", "ml.c5.large", 4)])
            .await
            .1;
        let logical_ids: Vec<String> = picks
            .iter()
            .map(|at| nodes[*at].logical_id.clone())
            .collect();
        let rebooted = emulator
            .ok(
                "BatchRebootClusterNodes",
                &json!({ "ClusterName": cluster, "NodeLogicalIds": logical_ids }),
            )
            .await;
        assert_eq!(
            string_list(&rebooted, "SuccessfulNodeLogicalIds"),
            Some(logical_ids.clone()),
            "case {case:?} by logical id: SuccessfulNodeLogicalIds must carry the requested ids \
             in order, got {rebooted}"
        );
        assert_eq!(
            rebooted.get("FailedNodeLogicalIds"),
            Some(&json!([])),
            "case {case:?} by logical id: no requested id failed, got {rebooted}"
        );
        assert_eq!(
            rebooted.get("Successful"),
            None,
            "case {case:?} by logical id: the instance-id lists belong to a request that named \
             instance ids, got {rebooted}"
        );
        assert_eq!(
            rebooted.get("Failed"),
            None,
            "case {case:?} by logical id: the instance-id lists belong to a request that named \
             instance ids, got {rebooted}"
        );
    }
}

/// RULE: a reboot is a soft recovery of the instance the node ALREADY runs on — it "attempts
/// to cleanly shut down the operating system before restarting the instance" — so the node
/// keeps its `NodeLogicalId`, its `InstanceId`, its `LaunchTime` and its configuration, and
/// reports `Running` again once the clock owes it back.
#[tokio::test]
async fn a_rebooted_node_comes_back_on_the_instance_it_was_already_running_on() {
    let emulator = Emulator::new();
    let cluster = "reboot-same-instance";
    let nodes = in_service_cluster(
        &emulator,
        cluster,
        &[("controller", "ml.c5.large", 1), ("workers", "ml.g5.xlarge", 2)],
    )
    .await
    .1;
    // Rows differ along which identifier kind named the node, and which instance group it
    // belongs to.
    let rows: Vec<(&str, usize, bool)> = vec![
        ("by-instance-id", 0, true),
        ("by-logical-id", 1, false),
        ("other-group-by-logical-id", 2, false),
    ];
    for (case, at, by_instance_id) in rows {
        let before = nodes[at].clone();
        let request = if by_instance_id {
            json!({ "ClusterName": cluster, "NodeIds": [before.instance_id.clone()] })
        } else {
            json!({ "ClusterName": cluster, "NodeLogicalIds": [before.logical_id.clone()] })
        };
        emulator.ok("BatchRebootClusterNodes", &request).await;

        // While the restart is outstanding the node is not serving, and it is still the
        // same instance: a reboot terminates nothing.
        let during = describe(&emulator, cluster, &before.logical_id).await;
        assert_eq!(
            during.get("InstanceId").and_then(Value::as_str),
            Some(before.instance_id.as_str()),
            "case {case:?}: a reboot keeps the instance the node runs on, got {during}"
        );
        assert_eq!(
            str_field(&during["InstanceStatus"], "Status"),
            "Pending",
            "case {case:?}: a node whose restart is outstanding is not serving, got {during}"
        );

        let after = node_once_running(&emulator, cluster, &before.logical_id).await;
        assert_eq!(
            after,
            Node {
                status: "Running".to_string(),
                ..before.clone()
            },
            "case {case:?}: the rebooted node is the one it was, on the same instance and \
             launch time, got {after:?}"
        );
    }
}

/// RULE: an identifier naming no node of the cluster is a PER-NODE failure — reported in the
/// list of its own identifier kind with the modelled code and the wording of that list's
/// error shape — so the identifiers beside it in the same batch are still rebooted and the
/// call is not refused; a request NONE of whose identifiers names a node asked for nothing
/// the call can do and is refused outright instead.
#[tokio::test]
async fn an_identifier_naming_no_node_is_reported_beside_the_nodes_that_rebooted() {
    // Rows differ along the identifier kind, the form of the unknown identifier (a long and
    // a short instance id, a made-up logical id) and whether any identifier of the request
    // named a node at all.
    let rows: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("unknown-long-instance-id", vec!["i-0123456789abcdef0"], vec![]),
        ("unknown-short-instance-id", vec!["i-abcdef01"], vec![]),
        ("unknown-logical-id", vec![], vec!["no-such-node"]),
        (
            "unknown-of-both-kinds",
            vec!["i-0123456789abcdef0"],
            vec!["no-such-node"],
        ),
        (
            "several-unknown-instance-ids",
            vec!["i-0123456789abcdef0", "i-abcdef01", "i-00000000000000001"],
            vec![],
        ),
    ];
    for (case, unknown_ids, unknown_logical) in rows {
        let emulator = Emulator::new();
        let cluster = format!("reboot-unknown-{case}");
        let nodes = in_service_cluster(&emulator, &cluster, &[("pool", "ml.c5.large", 1)])
            .await
            .1;
        let known = nodes[0].clone();
        let mut node_ids: Vec<String> = unknown_ids.iter().map(|id| (*id).to_string()).collect();
        let logical_ids: Vec<String> = unknown_logical
            .iter()
            .map(|id| (*id).to_string())
            .collect();
        // One identifier the cluster DOES hold, so the call has something to do and the
        // unknown ones beside it are answered per node rather than refusing the request.
        node_ids.push(known.instance_id.clone());
        let mut request = json!({ "ClusterName": cluster, "NodeIds": node_ids.clone() });
        if !logical_ids.is_empty() {
            request["NodeLogicalIds"] = json!(logical_ids.clone());
        }
        let answered = emulator.ok("BatchRebootClusterNodes", &request).await;

        let expected_failed: Vec<Value> = unknown_ids
            .iter()
            .map(|id| {
                json!({ "NodeId": id, "ErrorCode": "InstanceIdNotFound", "Message": INSTANCE_NOT_FOUND })
            })
            .collect();
        assert_eq!(
            answered.get("Failed"),
            Some(&json!(expected_failed)),
            "case {case:?}: every instance id naming no node is reported in Failed, got \
             {answered}"
        );
        let expected_failed_logical: Vec<Value> = unknown_logical
            .iter()
            .map(|id| {
                json!({
                    "NodeLogicalId": id,
                    "ErrorCode": "InstanceIdNotFound",
                    "Message": NODE_NOT_FOUND,
                })
            })
            .collect();
        assert_eq!(
            answered.get("FailedNodeLogicalIds"),
            (!logical_ids.is_empty()).then_some(&json!(expected_failed_logical)),
            "case {case:?}: every logical id naming no node is reported in \
             FailedNodeLogicalIds, got {answered}"
        );
        assert_eq!(
            string_list(&answered, "Successful"),
            Some(vec![known.instance_id.clone()]),
            "case {case:?}: the identifiers beside the unknown ones are still rebooted, got \
             {answered}"
        );

        // The cluster holds exactly the node it held: an unknown identifier added none.
        let after = nodes_of(&emulator, &cluster).await;
        assert_eq!(
            after.len(),
            1,
            "case {case:?}: an unknown identifier creates no node, got {after:?}"
        );
    }
}

/// RULE: a request EVERY identifier of which names no node of the cluster asked for nothing
/// the call can do — the failure lists report the nodes it could not reboot beside the ones
/// it could, and with nothing rebooted there is no such answer to give — so it is refused
/// outright rather than answered with a 200 whose every entry is a failure.
#[tokio::test]
async fn a_request_naming_no_node_of_the_cluster_is_refused_outright() {
    let emulator = Emulator::new();
    let cluster = "reboot-nothing-held";
    let nodes = in_service_cluster(&emulator, cluster, &[("pool", "ml.c5.large", 2)])
        .await
        .1;
    // A node the cluster held until a delete made the caller's node list stale: its
    // identifiers name no node of the cluster any more.
    let removed = nodes[1].clone();
    emulator
        .ok(
            "BatchDeleteClusterNodes",
            &json!({ "ClusterName": cluster, "NodeIds": [removed.instance_id.clone()] }),
        )
        .await;
    let surviving = nodes_of(&emulator, cluster).await;
    // Rows differ along the identifier kind and form, and along whether the node never
    // existed or was deleted after being listed.
    let rows: Vec<(&str, Value)> = vec![
        (
            "stale-instance-id-of-a-deleted-node",
            json!({ "ClusterName": cluster, "NodeIds": [removed.instance_id.clone()] }),
        ),
        (
            "stale-logical-id-of-a-deleted-node",
            json!({ "ClusterName": cluster, "NodeLogicalIds": [removed.logical_id.clone()] }),
        ),
        (
            "one-unknown-instance-id",
            json!({ "ClusterName": cluster, "NodeIds": ["i-0123456789abcdef0"] }),
        ),
        (
            "several-unknown-instance-ids",
            json!({
                "ClusterName": cluster,
                "NodeIds": ["i-0123456789abcdef0", "i-abcdef01", "i-00000000000000001"],
            }),
        ),
        (
            "one-unknown-logical-id",
            json!({ "ClusterName": cluster, "NodeLogicalIds": ["no-such-node"] }),
        ),
        (
            "both-kinds-unknown",
            json!({
                "ClusterName": cluster,
                "NodeIds": ["i-0123456789abcdef0"],
                "NodeLogicalIds": [removed.logical_id.clone(), "no-such-node"],
            }),
        ),
    ];
    for (case, request) in rows {
        let (status, body) = emulator.call("BatchRebootClusterNodes", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        assert_eq!(
            message(&body),
            NONE_EXIST,
            "case {case:?}: expected the all-missing refusal, got {body}"
        );
    }

    // None of the refusals touched the cluster: the surviving node is still the one it was,
    // on the instance it was already running on.
    let after = nodes_of(&emulator, cluster).await;
    assert_eq!(
        after, surviving,
        "case refusals: a refused request reboots nothing, got {after:?}"
    );
}

/// RULE: a node the service is already changing cannot take a reboot — a node with an
/// operation of its own in flight is reported `InstanceIdInUse` ("another operation is
/// already in progress for this node") and one that has not finished coming up
/// `InvalidInstanceStatus` — so a reboot never interrupts a change already under way.
#[tokio::test]
async fn a_node_already_mid_change_is_reported_rather_than_rebooted_again() {
    let emulator = Emulator::new();
    let cluster = "reboot-mid-change";
    let nodes = in_service_cluster(&emulator, cluster, &[("pool", "ml.c5.large", 2)])
        .await
        .1;
    let node = nodes[0].clone();
    emulator
        .ok(
            "BatchRebootClusterNodes",
            &json!({ "ClusterName": cluster, "NodeIds": [node.instance_id.clone()] }),
        )
        .await;
    // The same node again, by each identifier kind, while its first reboot is still
    // outstanding.
    let again = emulator
        .ok(
            "BatchRebootClusterNodes",
            &json!({
                "ClusterName": cluster,
                "NodeIds": [node.instance_id.clone()],
                "NodeLogicalIds": [node.logical_id.clone()],
            }),
        )
        .await;
    assert_eq!(
        again.get("Failed"),
        Some(&json!([{
            "NodeId": node.instance_id.clone(),
            "ErrorCode": "InstanceIdInUse",
            "Message": IN_USE,
        }])),
        "case in-flight: a second reboot of the same node is refused per node, got {again}"
    );
    assert_eq!(
        again.get("FailedNodeLogicalIds"),
        Some(&json!([{
            "NodeLogicalId": node.logical_id.clone(),
            "ErrorCode": "InstanceIdInUse",
            "Message": IN_USE,
        }])),
        "case in-flight: the logical id of the same node is refused the same way, got {again}"
    );
    assert_eq!(
        again.get("Successful"),
        None,
        "case in-flight: nothing was rebooted a second time, and an empty Successful is not a \
         modelled value, got {again}"
    );
    assert_eq!(
        again.get("SuccessfulNodeLogicalIds"),
        None,
        "case in-flight: nothing was rebooted a second time, and an empty \
         SuccessfulNodeLogicalIds is not a modelled value, got {again}"
    );

    // A node whose hardware is being replaced is mid-change too, and the replacement is
    // what a reboot must not interrupt.
    let replacing = nodes[1].clone();
    emulator
        .ok(
            "BatchReplaceClusterNodes",
            &json!({ "ClusterName": cluster, "NodeIds": [replacing.instance_id.clone()] }),
        )
        .await;
    let refused = emulator
        .ok(
            "BatchRebootClusterNodes",
            &json!({ "ClusterName": cluster, "NodeIds": [replacing.instance_id.clone()] }),
        )
        .await;
    assert_eq!(
        refused.get("Failed"),
        Some(&json!([{
            "NodeId": replacing.instance_id.clone(),
            "ErrorCode": "InstanceIdInUse",
            "Message": IN_USE,
        }])),
        "case replacing: a node whose replacement is in flight takes no reboot, got {refused}"
    );

    // A node that is still coming up has not settled either, so it cannot be rebooted until
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
        .ok(
            "BatchRebootClusterNodes",
            &json!({ "ClusterName": cluster, "NodeLogicalIds": [provisioning.clone()] }),
        )
        .await;
    assert_eq!(
        refused.get("FailedNodeLogicalIds"),
        Some(&json!([{
            "NodeLogicalId": provisioning,
            "ErrorCode": "InvalidInstanceStatus",
            "Message": NODE_INVALID_STATUS,
        }])),
        "case provisioning: a node that has not finished coming up cannot be rebooted, got \
         {refused}"
    );
}

/// RULE: a request that cannot name the nodes to reboot is refused outright rather than
/// answered — it must name at least one identifier, and no more identifiers than one call
/// may reboot ("you can reboot up to 25 nodes in a single request") — while a request AT
/// that bound is answered.
#[tokio::test]
async fn a_request_that_cannot_name_the_nodes_to_reboot_is_refused() {
    let emulator = Emulator::new();
    let cluster = "reboot-refusals";
    let nodes = in_service_cluster(&emulator, cluster, &[("pool", "ml.c5.large", 1)])
        .await
        .1;
    let node = nodes[0].clone();
    // Instance ids in the modelled `ClusterNodeId` form (pattern
    // `^i-[a-f0-9]{8}(?:[a-f0-9]{9})?$`), so only the batch bound can refuse them.
    let ids = |count: usize| -> Vec<String> {
        (0..count).map(|at| format!("i-{at:08x}")).collect()
    };
    // Rows differ along how a request fails to name its nodes, and sit past the published
    // bound of 25 identifiers — by one list alone, which the model bounds at 25 EACH, and by
    // the two lists together.
    let rows: Vec<(&str, Value)> = vec![
        ("no-identifier", json!({ "ClusterName": cluster })),
        (
            "one-past-the-bound-of-one-list",
            json!({ "ClusterName": cluster, "NodeIds": ids(26) }),
        ),
        (
            "one-past-the-bound-across-both-lists",
            json!({
                "ClusterName": cluster,
                "NodeIds": ids(25),
                "NodeLogicalIds": ["no-such-node"],
            }),
        ),
        (
            "well-past-the-bound-across-both-lists",
            json!({
                "ClusterName": cluster,
                "NodeIds": ids(13),
                "NodeLogicalIds": (0..13).map(|at| format!("no-such-node-{at}")).collect::<Vec<_>>(),
            }),
        ),
    ];
    for (case, request) in rows {
        let (status, body) = emulator.call("BatchRebootClusterNodes", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        assert!(
            !message(&body).is_empty(),
            "case {case:?}: a refusal says what was wrong, got {body}"
        );
    }
    // None of the refusals touched the cluster: the node is still the one it was, on the
    // instance it was already running on.
    let after = nodes_of(&emulator, cluster).await;
    assert_eq!(
        after,
        vec![node.clone()],
        "case refusals: a refused request reboots nothing, got {after:?}"
    );

    // AT the bound, with one identifier the cluster holds among the twenty-five, the
    // request is answered rather than refused.
    let mut at_the_bound = ids(24);
    at_the_bound.push(node.instance_id.clone());
    let answered = emulator
        .ok(
            "BatchRebootClusterNodes",
            &json!({ "ClusterName": cluster, "NodeIds": at_the_bound }),
        )
        .await;
    assert_eq!(
        string_list(&answered, "Successful"),
        Some(vec![node.instance_id.clone()]),
        "case at-the-bound: a request of twenty-five identifiers is answered, got {answered}"
    );
}
