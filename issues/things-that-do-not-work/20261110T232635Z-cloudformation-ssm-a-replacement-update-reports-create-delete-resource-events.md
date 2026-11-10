Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ssm.cfn-ssm-parameter-create-update-in-place-replace-retained-then-delete
Issue-Key: cloudformation__ssm.cfn-ssm-parameter-create-update-in-place-replace-retained-then-delete|_assert_replaced_and_retained#29|AssertionError|assert _in_order(
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# A replacement update reports CREATE_*/DELETE_* resource events and deletes the old resource despite UpdateReplacePolicy Retain (2 causes)

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Two net-new root causes on CloudFormation's replacement-update path, both observed on one AWS::SSM::Parameter whose create-only Name property changes.

Cause 1 of 2: the resource events of a replacement are reported as a CREATE, not as an UPDATE.
AWS: Smithy (cloudformation) DescribeStackEvents -> StackEvents[*] with ResourceStatus, ResourceStatusReason and PhysicalResourceId. When an update changes a create-only property, the resource is REPLACED inside the update: AWS emits, for that logical id, UPDATE_IN_PROGRESS with ResourceStatusReason "Requested update requires the creation of a new physical resource; hence creating one." (fixed text), then UPDATE_IN_PROGRESS "Resource creation Initiated", then UPDATE_COMPLETE carrying the NEW PhysicalResourceId (here the new parameter name, a value from the request).
Emulator: the same update emits CREATE_IN_PROGRESS (ResourceStatusReason absent, PhysicalResourceId empty), CREATE_IN_PROGRESS "Resource creation Initiated", CREATE_COMPLETE - no UPDATE_* event and no replacement reason at all.
Why: a client that follows DescribeStackEvents to decide what an update did (console-style progress, CI gates, drift and audit tooling) cannot tell a replacement from a fresh create, and never sees the one reason string that announces data-bearing resources are about to be re-created.
Fix: the CloudFormation update engine's resource-event emission for the replacement path (status family and the replacement ResourceStatusReason), not the SSM parameter provider.

Cause 2 of 2: UpdateReplacePolicy Retain is ignored - the replaced physical resource is deleted.
AWS: a resource whose UpdateReplacePolicy is Retain is NOT deleted in the cleanup phase of a replacement; CloudFormation emits a DELETE_SKIPPED event carrying the OLD PhysicalResourceId and the old resource survives the update and the later DeleteStack (here GetParameter on the old name still answers its value and version).
Emulator: the cleanup phase emits DELETE_IN_PROGRESS (PhysicalResourceId absent) and then DELETE_COMPLETE for the OLD physical id - no DELETE_SKIPPED event, and the retained resource is destroyed.
Why: this is silent data loss on a documented retention guarantee - the whole point of UpdateReplacePolicy Retain is that the old resource (a parameter, a bucket, a table) outlives the replacement. A client relying on it loses the resource and cannot even detect the loss from the event stream, which reports a successful delete rather than a skip.
Fix: the CloudFormation update engine's cleanup phase - honour the resource's UpdateReplacePolicy (Retain/Snapshot/Delete) and emit DELETE_SKIPPED with the old physical id when it is Retain.

Traces: the single failing check below exercises both causes (it fails at the first).

## Evidence
Check cloudformation__ssm.cfn-ssm-parameter-create-update-in-place-replace-retained-then-delete

AssertionError: the replacement must report creating a new resource, complete as '/<prefix>/second' and skip deleting '/<prefix>/first' (UpdateReplacePolicy Retain), in that order: [('CREATE_IN_PROGRESS', None, ''), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', '/<prefix>/second'), ('CREATE_COMPLETE', None, '/<prefix>/second'), ('DELETE_IN_PROGRESS', None, None), ('DELETE_COMPLETE', None, '/<prefix>/first')]

Rows are (ResourceStatus, ResourceStatusReason, PhysicalResourceId) for the replaced logical id, oldest first, read through DescribeStackEvents.

Expected (AWS), in order:
  ('UPDATE_IN_PROGRESS', 'Requested update requires the creation of a new physical resource; hence creating one.', <any>)
  ('UPDATE_COMPLETE', None, '<new physical id>')
  ('DELETE_SKIPPED', None, '<old physical id>')
and NO DELETE_IN_PROGRESS / DELETE_COMPLETE event at all.

Actual (emulator): CREATE_IN_PROGRESS / CREATE_IN_PROGRESS('Resource creation Initiated') / CREATE_COMPLETE, then DELETE_IN_PROGRESS and DELETE_COMPLETE on the old physical id - the retained resource is deleted.

## Steps to Reproduce
1. CreateStack with one AWS::SSM::Parameter (Type String, a Name, a Value) that declares "UpdateReplacePolicy": "Retain".
2. UpdateStack with the same template but a different Name (Name is create-only, so this forces a replacement). Wait for UPDATE_COMPLETE.
3. DescribeStackEvents for the stack and filter to that logical id: observe CREATE_IN_PROGRESS/CREATE_COMPLETE instead of UPDATE_IN_PROGRESS with "Requested update requires the creation of a new physical resource; hence creating one." and UPDATE_COMPLETE.
4. Observe DELETE_IN_PROGRESS + DELETE_COMPLETE for the OLD parameter name instead of a single DELETE_SKIPPED, and ssm:GetParameter on the old name no longer answers.
