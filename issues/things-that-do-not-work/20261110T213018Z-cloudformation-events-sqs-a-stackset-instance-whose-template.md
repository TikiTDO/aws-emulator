Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__events__sqs.xsvc-cloudformation-2-stackset-failed-instance-operation-status
Issue-Key: cloudformation__events__sqs.xsvc-cloudformation-2-stackset-failed-instance-operation-status|run#19|AssertionError|assert op["Status"] == "FAILED", (
Issue-Commit: 3d64db7d7cd365a6872993643ce273c9a29364de

# cloudformation__events__sqs: a StackSet instance whose template cannot be created is reported SUCCEEDED — CreateStackInstances never reaches FAILED and the instance never becomes OUTDATED/FAILED

> _Note: this issue was identified against the code base at commit `3d64db7d7cd365a6872993643ce273c9a29364de`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStackInstances

## Description
Cause 1 of 1 — the StackSet orchestrator settles every CreateStackInstances operation as SUCCEEDED, even when the instance's own stack cannot be created, so a failed deployment is indistinguishable from a successful one.

Failing check ids for this cause: cloudformation__events__sqs.xsvc-cloudformation-2-stackset-failed-instance-operation-status

AWS: a SELF_MANAGED StackSet whose template declares an AWS::CloudFormation::WaitCondition (logical id W) with Timeout 1 and an AWS::CloudFormation::WaitConditionHandle that nobody signals cannot create: the instance's stack fails on the WaitCondition. With OperationPreferences FailureToleranceCount 0, one failed instance exceeds the tolerance, so the operation is terminal FAILED. Per the Smithy model (com.amazonaws.cloudformation): StackSetOperationStatus is an enum with values RUNNING | SUCCEEDED | FAILED | STOPPING | STOPPED | QUEUED, and DescribeStackSetOperation's StackSetOperation.StatusDetails is a StackSetOperationStatusDetails whose FailedStackInstancesCount (@range min 0) is the number of stack instances for which the operation failed — so for this operation AWS reports Status "FAILED" and StatusDetails.FailedStackInstancesCount 1 (the count is the number of failing instances for the request, not a literal). ListStackInstances then reports, for that instance, StackInstanceSummary.Status "OUTDATED" (the model documents OUTDATED as "the associated stack failed during a CreateStackSet or UpdateStackSet operation"; StackInstanceStatus enum is CURRENT | OUTDATED | INOPERABLE) and StackInstanceSummary.StackInstanceStatus.DetailedStatus "FAILED" (StackInstanceDetailedStatus enum includes PENDING | RUNNING | SUCCEEDED | FAILED | CANCELLED | ...), with StatusReason explaining the failure. Recorded behavior only for the StatusReason template: AWS words it as fixed text naming the failing template resource plus the resource's own failure message — it contains "ResourceLogicalId:<logical id>" (here ResourceLogicalId:W, taken from the request's template) and the WaitCondition's "WaitCondition timed out" text; the logical id is a value from the request, the "ResourceLogicalId:" prefix and the "WaitCondition timed out" phrase are the fixed parts. On the default event bus AWS then emits, for the same OperationId, "CloudFormation StackSet Operation Status Change" with detail.action CREATE and detail.status-details.status RUNNING followed by FAILED (and never SUCCEEDED for that operation id), and "CloudFormation StackSet StackInstance Status Change" with detail.status-details.status OUTDATED, detailed-status FAILED and status-reason equal to the ListStackInstances StatusReason above.

Emulator: DescribeStackSetOperation settles the CreateStackInstances operation as Status 'SUCCEEDED' with StatusDetails {'FailedStackInstancesCount': 0}, Action 'CREATE', and an EndTimestamp ~0.5 s after CreationTimestamp — i.e. the unsignalled WaitCondition is treated as created. The replay output shows the terminal operation record as SUCCEEDED/FailedStackInstancesCount 0, so the check stops at its first assertion (expected operation FAILED) and the downstream assertions on ListStackInstances (Status OUTDATED, DetailedStatus FAILED, StatusReason naming ResourceLogicalId:W and "WaitCondition timed out") and on the two FAILED EventBridge detail-types are never reached. Note that the emulator now does reach a terminal status quickly, which is a change from the already-open issue 20261026T235900Z-cloudformation-events-sqs-stackset-operations-never-settle-and.md (that issue is about the operation never settling at all and no StackSet/Operation/StackInstance event arriving, and its replay key is a receive_message timeout); that issue does not cover reporting a failed instance as SUCCEEDED, which is this defect.

Why: a client that gates on the StackSet operation's terminal status — a pipeline that proceeds on SUCCEEDED, an alarm that fires on FAILED, a rollout that checks StatusDetails.FailedStackInstancesCount or re-drives OUTDATED/FAILED instances — is told a deployment that did not happen succeeded. It then advances past a broken rollout, never retries the instance, and never sees the failure reason that names the offending template resource; the SUCCEEDED-only EventBridge rule in this scenario receives a delivery for an operation that in fact failed.

Fix: the StackSet operation orchestrator in the cloudformation crate — the stage that drives a stack instance's embedded stack to a terminal state must propagate that stack's CREATE_FAILED outcome into the stack instance (Status OUTDATED, StackInstanceStatus.DetailedStatus FAILED, StatusReason naming the failing logical resource) and into the parent operation (Status FAILED, StatusDetails.FailedStackInstancesCount incremented) honouring OperationPreferences.FailureToleranceCount; this includes handling AWS::CloudFormation::WaitCondition/WaitConditionHandle timeout in the stack provisioner rather than treating the resource as created. The same transition is what feeds the "CloudFormation StackSet Operation Status Change" (RUNNING then FAILED) and "CloudFormation StackSet StackInstance Status Change" (OUTDATED / FAILED / status-reason) publications onto the default event bus.

Traces: cloudformation__events__sqs.xsvc-cloudformation-2-stackset-failed-instance-operation-status

## Evidence
Check cloudformation__events__sqs.xsvc-cloudformation-2-stackset-failed-instance-operation-status, assertion at check line 77:

  assert op["Status"] == "FAILED"
  AssertionError: CreateStackInstances 'b1b8bd13-16de-43d8-9003-c1420f16c092' with a template whose resource cannot be created: expected operation FAILED, got 'SUCCEEDED'

Emulator's DescribeStackSetOperation result for that operation (per-run ids/timestamps elided as environment-specific):
  {'OperationId': <id>, 'StackSetId': '<set-name>:<uuid>', 'Action': 'CREATE', 'Status': 'SUCCEEDED',
   'OperationPreferences': {'FailureToleranceCount': 0, 'MaxConcurrentCount': 1},
   'AdministrationRoleARN': <admin role arn>, 'ExecutionRoleName': <exec role name>,
   'CreationTimestamp': <t>, 'EndTimestamp': <t + ~0.5s>,
   'StatusDetails': {'FailedStackInstancesCount': 0}, 'RetryUndoOps': []}

Expected (AWS), for the same request shape:
  DescribeStackSetOperation -> StackSetOperation.Status == "FAILED",
    StackSetOperation.Action == "CREATE",
    StackSetOperation.StatusDetails.FailedStackInstancesCount == <number of failed instances> (1 here, FailureToleranceCount 0 exceeded)
  ListStackInstances -> exactly one Summary with
    Status == "OUTDATED",
    StackInstanceStatus.DetailedStatus == "FAILED",
    StatusReason containing "ResourceLogicalId:<failing logical id>" (W, from the submitted template) and "WaitCondition timed out"
  default event bus -> "CloudFormation StackSet Operation Status Change" for that stack-set-operation-id with detail.action "CREATE" and detail.status-details.status "RUNNING" then "FAILED" (never "SUCCEEDED"), and "CloudFormation StackSet StackInstance Status Change" for the instance's stack-id with detail.status-details.status "OUTDATED", detailed-status "FAILED", status-reason equal to the ListStackInstances StatusReason.

No HTTP status or error code diverges here: every call returned 2xx; the divergence is the reported terminal status and the absent failure propagation.

## Steps to Reproduce
1. Create two SQS queues, each fed by an EventBridge rule on the default bus: a control rule matching source aws.cloudformation and detail-types 'CloudFormation StackSet Status Change', 'CloudFormation StackSet Operation Status Change', 'CloudFormation StackSet StackInstance Status Change' with detail.stack-set-arn prefixed by the stack set's ARN prefix; and a claim rule matching only detail-type 'CloudFormation StackSet Operation Status Change' with detail.status-details.status ["SUCCEEDED"].
2. Create a SELF_MANAGED administration role trusted by cloudformation.amazonaws.com and an execution role trusted by it.
3. CreateStackSet with PermissionModel SELF_MANAGED, AdministrationRoleARN/ExecutionRoleName above, and TemplateBody {"Resources": {"H": {"Type": "AWS::CloudFormation::WaitConditionHandle"}, "W": {"Type": "AWS::CloudFormation::WaitCondition", "Properties": {"Handle": {"Ref": "H"}, "Timeout": "1"}}}} — nothing ever signals the handle.
4. CreateStackInstances for the caller account and Region with OperationPreferences {FailureToleranceCount: 0, MaxConcurrentCount: 1}, then poll DescribeStackSetOperation until the Status leaves RUNNING/QUEUED/STOPPING.
5. Observe: the emulator returns Status 'SUCCEEDED' with StatusDetails.FailedStackInstancesCount 0. AWS returns Status 'FAILED' with FailedStackInstancesCount 1.
6. Call ListStackInstances: AWS reports the single instance as Status OUTDATED, StackInstanceStatus.DetailedStatus FAILED and a StatusReason containing 'ResourceLogicalId:W' and 'WaitCondition timed out'.
7. Drain the control queue: AWS delivers 'CloudFormation StackSet Operation Status Change' RUNNING then FAILED (action CREATE) for that OperationId plus 'CloudFormation StackSet StackInstance Status Change' with status OUTDATED / detailed-status FAILED and the same status-reason, and the SUCCEEDED-only claim queue receives nothing for that OperationId.
