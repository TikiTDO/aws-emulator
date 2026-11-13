Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.deregister-container-instance-requires-force
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.deregister-container-instance-requires-force|_error#17|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 9bd5b7da5aa90741deae627d938338ffb9a7a206

# ecs: DeregisterContainerInstance without force succeeds on a container instance that still has a task placed on it

> _Note: this issue was identified against the code base at commit `9bd5b7da5aa90741deae627d938338ffb9a7a206`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeregisterContainerInstance

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: DeregisterContainerInstance called WITHOUT `force` on a container instance that still has a task placed on it is accepted and deregisters the host, instead of being refused.

AWS: Smithy (ecs) DeregisterContainerInstance input {cluster: String, containerInstance: String (required), force: BoxedBoolean}; modelled errors AccessDeniedException(403), ClientException(400), ClusterNotFoundException(400), InvalidParameterException(400), ServerException(500). The model documents `force` as the flag that lets the call go through while tasks are on the host ("If you have tasks running on the container instance when you deregister it with the force option, these tasks remain running ... but they're orphaned"), and the operation doc recommends stopping all tasks before deregistration. The model does not spell out the refusal itself, so for that part this is recorded behavior only: real AWS refuses the force-less call on a busy instance with HTTP 400, error code `InvalidParameterException` and the message `Found running tasks on the instance.` — that message is entirely FIXED text for this error class (it carries no request- or state-derived values) — and leaves the instance's status at `ACTIVE`. Once the task reaches STOPPED, the identical force-less call succeeds, removes the instance from ListContainerInstances and reports `status: INACTIVE` from DescribeContainerInstances.

Emulator: the force-less call on an instance whose DescribeContainerInstances view shows a placed task (pendingTasksCount 1) returns HTTP 200 with a full ContainerInstance body already flipped to `status: 'INACTIVE'`, `agentConnected: false`. No error is raised at all, so there is no status, code or message to compare — the refusal is simply absent, and the host is deregistered out from under a live task.

Why: a client that relies on the force-less call as its safety interlock (the documented "stop the tasks first" workflow, and every operator script that deregisters hosts one at a time) silently orphans running work on the emulator: the call it expects to be refused with InvalidParameterException instead removes the instance from the cluster, and the client never learns that tasks were still placed. Capacity accounting and any subsequent placement decision are taken from a cluster state real AWS would never have allowed.

Fix: the DeregisterContainerInstance handler's validation — it must inspect the tasks placed on the resolved container instance and, when any are not STOPPED and `force` is not true, refuse with InvalidParameterException and leave the instance's status/version untouched; only `force: true` (or an idle instance) may perform the ACTIVE -> INACTIVE transition.

Traces: the failing check id in trace_ids. File/line citation for the fixer: services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/deregister_container_instance.rs lines 206-249 — after resolving the ARN the handler sets `instance.status = CONTAINER_INSTANCE_INACTIVE` unconditionally and its comment states "`force` changes nothing here"; `input.force` is never read and no task lookup is performed. No open or fixed issue carries this operation's force interlock (the open ecs container-instance filings 20261107T080429Z-ecs-2-net-new-causes-no-aws-signed.md and 20261109T190659Z-ecs-putattributes-10-attributes-per-target-ceiling-is.md are about agent identity documents and PutAttributes limits, not this refusal).

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator.

[mismatch] ecs.deregister-container-instance-requires-force
  File "check_ecs_deregister_container_instance_requires_force.py", line 210, in run -> code, message = _error(lambda: ecs.deregister_container_instance(cluster=name, containerInstance=ci_arn))
  File "check_ecs_deregister_container_instance_requires_force.py", line 68, in _error -> raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
  AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'containerInstance': {... 'status': 'INACTIVE', 'agentConnected': False, 'runningTasksCount': 0, 'pendingTasksCount': 1 ...}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Expected (AWS): HTTP 400, Error.Code = 'InvalidParameterException', Error.Message = 'Found running tasks on the instance.' (fixed text for this class), and DescribeContainerInstances still reporting status 'ACTIVE' for the instance.
Actual (emulator): HTTP 200 with the deregistered ContainerInstance, status already 'INACTIVE' and agentConnected false, while the task placed by StartTask is still non-STOPPED (pendingTasksCount 1).

Smithy consulted (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/smithy-model/com-amazonaws-ecs-amazonec2containerservicev20141113.json): DeregisterContainerInstance errors include InvalidParameterException (HTTP 400); DeregisterContainerInstanceRequest$force documents the orphaning semantics of the forced form.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; the check id in trace_ids reproduces.
2. CreateCluster, then register/launch a container instance into that cluster.
3. RegisterTaskDefinition with one essential container and StartTask onto that container instance; poll DescribeTasks until lastStatus is PENDING/ACTIVATING/RUNNING.
4. Call DeregisterContainerInstance(cluster=<cluster>, containerInstance=<ci arn>) with NO force flag.
5. Expected: HTTP 400 InvalidParameterException 'Found running tasks on the instance.' and DescribeContainerInstances still ACTIVE. Actual: HTTP 200 and the instance is INACTIVE.
6. StopTask, wait for STOPPED, and call the same force-less DeregisterContainerInstance: it must now succeed, drop the ARN from ListContainerInstances and report INACTIVE.
7. Re-replay the id in trace_ids to confirm the fix.
