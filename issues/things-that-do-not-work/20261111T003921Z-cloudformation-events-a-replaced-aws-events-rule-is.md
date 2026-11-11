Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__events.cfn-events-rule-create-update-in-place-replace-retained-then-delete
Issue-Key: cloudformation__events.cfn-events-rule-create-update-in-place-replace-retained-then-delete|_assert_replaced_and_retained#21|AssertionError|assert _in_order(
Issue-Commit: b26d594b442f382e51363fa18dcb67bd5cc86e1e

# cloudformation__events: a replaced AWS::Events::Rule is reported with CREATE_*/DELETE_* events and the retained old rule is deleted despite UpdateReplacePolicy Retain (2 causes)

> _Note: this issue was identified against the code base at commit `b26d594b442f382e51363fa18dcb67bd5cc86e1e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Two net-new root causes, both observed on one AWS::Events::Rule whose create-only Name property changes during an update. The failing check is marked neither ALREADY FILED nor OWNED.

Cause 1 of 2: the resource events of a replacement are reported as a CREATE, not as an UPDATE, and the replacement reason is missing.
AWS: Smithy (cloudformation) DescribeStackEvents -> StackEvent{ResourceStatus, ResourceStatusReason, PhysicalResourceId}, where ResourceStatus is an enum that includes UPDATE_IN_PROGRESS, UPDATE_COMPLETE and DELETE_SKIPPED. When an update changes a create-only property the resource is REPLACED inside that update: AWS emits, for that logical id, UPDATE_IN_PROGRESS with ResourceStatusReason 'Requested update requires the creation of a new physical resource; hence creating one.' (fixed text), then UPDATE_IN_PROGRESS 'Resource creation Initiated', then UPDATE_COMPLETE carrying the NEW PhysicalResourceId (here the new rule name, a value from the request).
Emulator: the same update emits CREATE_IN_PROGRESS (ResourceStatusReason absent, PhysicalResourceId empty), CREATE_IN_PROGRESS 'Resource creation Initiated', CREATE_COMPLETE - no UPDATE_* event and no replacement reason at all.
Why: a client that follows DescribeStackEvents to decide what an update did (console-style progress, CI gates, drift and audit tooling) cannot tell a replacement from a fresh create and never sees the one reason string that announces an existing resource is about to be re-created.
Fix: the CloudFormation update engine's resource-event emission on the replacement path (status family plus the replacement ResourceStatusReason), not the EventBridge rule provisioner.
Traces: cloudformation__events.cfn-events-rule-create-update-in-place-replace-retained-then-delete.

Cause 2 of 2: UpdateReplacePolicy Retain is ignored - the replaced rule is deleted.
AWS: a resource whose UpdateReplacePolicy is Retain is NOT deleted in the cleanup phase of a replacement; CloudFormation emits a DELETE_SKIPPED event (a modeled ResourceStatus value) carrying the OLD PhysicalResourceId, and the old rule survives the update and the later DeleteStack - events DescribeRule on the old name still answers.
Emulator: the cleanup phase emits DELETE_IN_PROGRESS (PhysicalResourceId absent) and then DELETE_COMPLETE for the OLD physical id - no DELETE_SKIPPED event, and the retained rule is destroyed.
Why: this is silent loss of a resource on a documented retention guarantee. The point of UpdateReplacePolicy Retain is that the old resource outlives the replacement; a client relying on it loses the rule and cannot even detect the loss from the event stream, which reports a successful delete rather than a skip.
Fix: the CloudFormation update engine's cleanup phase - honour the resource's UpdateReplacePolicy (Retain/Snapshot/Delete) and emit DELETE_SKIPPED with the old physical id when it is Retain.
Traces: cloudformation__events.cfn-events-rule-create-update-in-place-replace-retained-then-delete (the check fails at Cause 1 and the event rows it prints also prove Cause 2).

The same engine-level behaviour is already recorded for AWS::SSM::Parameter in the open issue 20261110T232635Z-cloudformation-ssm-a-replacement-update-reports-create-delete-resource-events.md; its replay set names only the ssm check, so this recorded unit is not re-replayed by it.

## Evidence
Replay output (behavioral suite, section ### cloudformation__events):
  [mismatch] cloudformation__events.cfn-events-rule-create-update-in-place-replace-retained-then-delete
  AssertionError: the replacement must report creating a new resource, complete as '<new rule name>' and skip deleting '<old rule name>' (UpdateReplacePolicy Retain), in that order: [('CREATE_IN_PROGRESS', None, ''), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', '<new rule name>'), ('CREATE_COMPLETE', None, '<new rule name>'), ('DELETE_IN_PROGRESS', None, None), ('DELETE_COMPLETE', None, '<old rule name>')]

Rows are (ResourceStatus, ResourceStatusReason, PhysicalResourceId) for the replaced logical id, oldest first, read through DescribeStackEvents. The rule names are per-run request values.

Expected (AWS), in order:
  ('UPDATE_IN_PROGRESS', 'Requested update requires the creation of a new physical resource; hence creating one.', <any>)
  ('UPDATE_COMPLETE', None, '<new physical id>')
  ('DELETE_SKIPPED', None, '<old physical id>')
and NO DELETE_IN_PROGRESS / DELETE_COMPLETE event for that logical id.
Actual (emulator): CREATE_IN_PROGRESS / CREATE_IN_PROGRESS('Resource creation Initiated') / CREATE_COMPLETE, then DELETE_IN_PROGRESS and DELETE_COMPLETE on the old physical id.

## Steps to Reproduce
1. Build and boot the emulator from this checkout (cloudformation and events are both bootstrapped and serving).
2. CreateStack with one AWS::Events::Rule (a pinned Name, an EventPattern) that declares "UpdateReplacePolicy": "Retain".
3. UpdateStack with the same template but a different Name (Name is create-only, so this forces a replacement). Wait for UPDATE_COMPLETE.
4. DescribeStackEvents and filter to that logical id: expect UPDATE_IN_PROGRESS with 'Requested update requires the creation of a new physical resource; hence creating one.' then UPDATE_COMPLETE with the new rule name; observe CREATE_IN_PROGRESS/CREATE_COMPLETE with no reason instead.
5. Expect a single DELETE_SKIPPED event carrying the OLD rule name and events DescribeRule(Name=<old>) still answering; observe DELETE_IN_PROGRESS + DELETE_COMPLETE for the old name and the rule gone.
6. Equivalently: re-replay cloudformation__events.cfn-events-rule-create-update-in-place-replace-retained-then-delete.
