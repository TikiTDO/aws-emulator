Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.batch-delete-cluster-nodes-not-found sagemaker.batch-delete-cluster-nodes-partial-not-found sagemaker.batch-replace-cluster-nodes-stale-reference-rejected sagemaker.notebook-lifecycle-config-applied-server-side
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.batch-delete-cluster-nodes-not-found|run#20|AssertionError|assert bogus_failure["Code"] == "InstanceIdNotFound", (
Issue-Key: sagemaker.batch-delete-cluster-nodes-partial-not-found|run#20|AssertionError|assert failed[bogus_instance_id]["Code"] == "InstanceIdNotFound", failed
Issue-Key: sagemaker.batch-replace-cluster-nodes-stale-reference-rejected|_wait_for_node_presence#13|Timeout|time.sleep(15)
Issue-Key: sagemaker.notebook-lifecycle-config-applied-server-side|run#17|Timeout|time.sleep(15)
Issue-Commit: ea8447640442a063890341d04cda7629f55a3fd9

# SageMaker HyperPod and notebook paths diverge: BatchDeleteClusterNodes mis-codes an unknown NodeId, a created cluster never becomes InService, and a notebook lifecycle OnCreate hook is never executed (3 causes)

> _Note: this issue was identified against the code base at commit `ea8447640442a063890341d04cda7629f55a3fd9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDeleteClusterNodes

## Description
Three distinct root causes in the SageMaker service, each grounded in failing behavioral checks that are NOT marked ALREADY FILED or OWNED in this replay. The open issue 20261030T093739Z-sagemaker-inference-recommendation-refusal-texts-an-archived-model.md mentions unwired HyperPod batch node operations and 20261029T222441Z-sagemaker-batchdeleteclusternodes-stub.md names that operation, but neither carries the replay keys below; they are named here for cross-reference only. A fixer must close all three causes before submitting.

Cause 1 of 3 — BatchDeleteClusterNodes reports the wrong per-item error code and message for a well-formed but unknown node id passed in NodeIds.
AWS: recorded behavior only (both checks state the expectation was measured against real AWS, not taken from the model): for an id supplied via the NodeIds field that does not exist in the cluster, the per-item entry in Failed carries Code "InstanceIdNotFound" and Message "Node does not exist in the given cluster." (fixed text; NodeId is echoed from the request), while real ids in the same call still appear in Successful. Smithy: BatchDeleteClusterNodesError requires Code/Message/NodeId and the BatchDeleteClusterNodesErrorCode enum lists only NodeIdNotFound|InvalidNodeStatus|NodeIdInUse — so the model's enum does not cover the value real AWS returns for NodeIds, and the recorded behavior is the requirement.
Emulator: the Failed entry is {'Code': 'NodeIdNotFound', 'Message': 'The node does not exist in the specified cluster.', 'NodeId': '<echoed id>'} — wrong code and emulator-authored message wording ("The node does not exist in the specified cluster." instead of "Node does not exist in the given cluster.").
Why: a client branching on the per-item Code to distinguish an unknown instance id from an in-use or wrong-state node takes the wrong branch, and a client matching the documented message text never matches.
Fix: the BatchDeleteClusterNodes handler's per-item failure classification and message rendering (services/service-com-amazonaws-sagemaker-sagemaker/src/ops/batch_delete_cluster_nodes.rs).
Traces: sagemaker.batch-delete-cluster-nodes-not-found, sagemaker.batch-delete-cluster-nodes-partial-not-found.

Cause 2 of 3 — a created HyperPod cluster never reaches a terminal ClusterStatus, so node-level operations are unreachable.
AWS: Smithy: DescribeCluster.ClusterStatus enum Creating|Deleting|Failed|InService|RollingBack|SystemUpdating|Updating; after CreateCluster a valid Slurm cluster converges to InService and only then accepts BatchDeleteClusterNodes/BatchReplaceClusterNodes, where a BatchReplaceClusterNodes naming only an already-removed NodeLogicalId is refused as a whole with ValidationException and the message "All provided nodes do not exist".
Emulator: the check never gets that far — it polls DescribeCluster for InService|Failed and the check is killed by the suite's 60s budget with no terminal status observed, so the ValidationException boundary is never exercised. Hypothesis: the emulator leaves a created cluster in Creating indefinitely because nothing drives the cluster state machine.
Why: every HyperPod node operation is gated on an InService cluster, so a client that creates a cluster and waits for it hangs forever instead of proceeding or seeing a terminal Failed.
Fix: the CreateCluster handler and the cluster state machine behind DescribeCluster/ListClusterNodes (src/ops/create_cluster.rs and the cluster state component), plus the BatchReplaceClusterNodes whole-request refusal once a cluster is reachable (src/ops/batch_replace_cluster_nodes.rs).
Traces: sagemaker.batch-replace-cluster-nodes-stale-reference-rejected.

Cause 3 of 3 — a NotebookInstance's lifecycle-config OnCreate hook is never executed server-side and emits no log stream.
AWS: recorded behavior only (no Smithy member describes the hook execution): a NotebookInstance created with LifecycleConfigName reaches NotebookInstanceStatus=InService only after the platform has actually run the OnCreate script, and the script's stdout is observable in the CloudWatch Logs stream the platform emits for that hook.
Emulator: the check polls DescribeNotebookInstance and the hook's log stream for the marker the script echoes and never observes it, so the check is killed by the suite's 60s budget with the marker absent. Hypothesis: the emulator stores the lifecycle config but has no runtime that executes the OnCreate hook and never creates its log stream.
Why: a client that provisions notebooks via a lifecycle script (installing packages, mounting data) believes the script ran because the instance eventually reports InService, while nothing was executed and no log exists to diagnose it.
Fix: the CreateNotebookInstance handler and the notebook lifecycle-config execution/state component that must run the OnCreate hook and publish its log stream (src/ops/create_notebook_instance.rs plus the notebook state component).
Traces: sagemaker.notebook-lifecycle-config-applied-server-side.

## Evidence
Replay output (verbatim tails):
- sagemaker.batch-delete-cluster-nodes-not-found: AssertionError: expected Failed[i-fffffffffffffffff].Code == 'InstanceIdNotFound', got {'Code': 'NodeIdNotFound', 'Message': 'The node does not exist in the specified cluster.', 'NodeId': 'i-fffffffffffffffff'} (the check then also asserts Message == "Node does not exist in the given cluster.")
- sagemaker.batch-delete-cluster-nodes-partial-not-found: AssertionError: {'i-00000000deadbeef0': {'Code': 'NodeIdNotFound', 'Message': 'The node does not exist in the specified cluster.', 'NodeId': 'i-00000000deadbeef0'}} — expected Code 'InstanceIdNotFound' and Message 'Node does not exist in the given cluster.', with the real node id still in Successful
- sagemaker.batch-replace-cluster-nodes-stale-reference-rejected: Error: Check timed out after 60.0s — the check's only unbounded wait is _wait_for_cluster_status(sm, cluster, {"InService","Failed"}, timeout_s=1800) polling DescribeCluster every 30s, so no terminal ClusterStatus was observed; the ValidationException "All provided nodes do not exist" assertion was never reached
- sagemaker.notebook-lifecycle-config-applied-server-side: Error: Check timed out after 60.0s — the check loops for up to 900s polling DescribeNotebookInstance and _log_stream_contains(...marker...) for the marker echoed by the OnCreate script; the marker never appeared

Smithy (sagemaker model): BatchDeleteClusterNodesErrorCode = [NodeIdNotFound, InvalidNodeStatus, NodeIdInUse]; BatchDeleteClusterNodesError members Code/Message/NodeId all required, NodeId pattern ^i-[a-f0-9]{8}(?:[a-f0-9]{9})?$.

## Steps to Reproduce
1. Create a Slurm HyperPod cluster (controller + worker group), wait for ClusterStatus=InService, add one node, then call BatchDeleteClusterNodes with NodeIds = [<real instance id>, i-fffffffffffffffff]: the Failed entry for the unknown id reports Code 'NodeIdNotFound' and Message 'The node does not exist in the specified cluster.' instead of 'InstanceIdNotFound' / 'Node does not exist in the given cluster.'.
2. CreateCluster and poll DescribeCluster for a terminal ClusterStatus: no InService (or Failed) is observed, so BatchReplaceClusterNodes against an already-deleted NodeLogicalId can never be exercised for its whole-request ValidationException "All provided nodes do not exist".
3. CreateNotebookInstanceLifecycleConfig with an OnCreate script that echoes a marker, CreateNotebookInstance with that LifecycleConfigName, then poll DescribeNotebookInstance and the hook's CloudWatch Logs stream: the marker never appears, i.e. the hook was never executed.
