Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__logs.cfn-logs-log-group-create-update-in-place-replace-retained-then-delete
Issue-Key: cloudformation__logs.cfn-logs-log-group-create-update-in-place-replace-retained-then-delete|_assert_replaced_and_retained#24|AssertionError|assert _in_order(
Issue-Commit: b26d594b442f382e51363fa18dcb67bd5cc86e1e

# cloudformation__logs: a replaced AWS::Logs::LogGroup is reported with CREATE_*/DELETE_* events and the retained old log group is deleted despite UpdateReplacePolicy Retain (2 causes)

> _Note: this issue was identified against the code base at commit `b26d594b442f382e51363fa18dcb67bd5cc86e1e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Two net-new root causes, both observed on one AWS::Logs::LogGroup whose create-only LogGroupName changes during an update. The failing check is marked neither ALREADY FILED nor OWNED.

Cause 1 of 2: the resource events of a replacement are reported as a CREATE, not as an UPDATE, and the replacement reason is missing.
AWS: Smithy (cloudformation) DescribeStackEvents -> StackEvent{ResourceStatus, ResourceStatusReason, PhysicalResourceId}; ResourceStatus is an enum that includes UPDATE_IN_PROGRESS, UPDATE_COMPLETE and DELETE_SKIPPED. An update that changes a create-only property REPLACES the resource inside the update: AWS emits UPDATE_IN_PROGRESS with ResourceStatusReason 'Requested update requires the creation of a new physical resource; hence creating one.' (fixed text), then UPDATE_IN_PROGRESS 'Resource creation Initiated', then UPDATE_COMPLETE carrying the NEW PhysicalResourceId (here the new log-group name, a request value).
Emulator: CREATE_IN_PROGRESS (reason absent, PhysicalResourceId empty), CREATE_IN_PROGRESS 'Resource creation Initiated', CREATE_COMPLETE - no UPDATE_* event and no replacement reason.
Why: a client reading DescribeStackEvents cannot tell a replacement from a fresh create and never sees the reason that announces a log group holding ingested data is about to be re-created.
Fix: the CloudFormation update engine's resource-event emission on the replacement path, not the CloudWatch Logs provisioner.
Traces: cloudformation__logs.cfn-logs-log-group-create-update-in-place-replace-retained-then-delete.

Cause 2 of 2: UpdateReplacePolicy Retain is ignored - the replaced log group is deleted.
AWS: with UpdateReplacePolicy Retain the old physical resource is NOT deleted in the replacement's cleanup phase; CloudFormation emits DELETE_SKIPPED (a modeled ResourceStatus value) carrying the OLD PhysicalResourceId, and the old log group - with its ingested events - survives the update and the later DeleteStack; logs DescribeLogGroups/GetLogEvents on the old name still answer.
Emulator: DELETE_IN_PROGRESS (PhysicalResourceId absent) then DELETE_COMPLETE for the OLD log-group name - no DELETE_SKIPPED, and the retained log group and everything in it are destroyed.
Why: this is silent DATA loss on a documented retention guarantee - retaining log groups across replacements is exactly why operators set UpdateReplacePolicy Retain, and the event stream reports a successful delete rather than a skip, so the loss cannot be detected from the stack's record.
Fix: the CloudFormation update engine's cleanup phase - honour UpdateReplacePolicy (Retain/Snapshot/Delete) and emit DELETE_SKIPPED with the old physical id when it is Retain.
Traces: cloudformation__logs.cfn-logs-log-group-create-update-in-place-replace-retained-then-delete (the check fails at Cause 1; its printed event rows also prove Cause 2).

The same engine-level behaviour is already recorded for AWS::SSM::Parameter in the open issue 20261110T232635Z-cloudformation-ssm-a-replacement-update-reports-create-delete-resource-events.md; its replay set names only the ssm check, so this recorded unit is not re-replayed by it.

## Evidence
Replay output (behavioral suite, section ### cloudformation__logs):
  [mismatch] cloudformation__logs.cfn-logs-log-group-create-update-in-place-replace-retained-then-delete
  AssertionError: the replacement must report creating a new resource, complete as '/<new group>' and skip deleting '/<old group>' (UpdateReplacePolicy Retain), in that order: [('CREATE_IN_PROGRESS', None, ''), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', '/<new group>'), ('CREATE_COMPLETE', None, '/<new group>'), ('DELETE_IN_PROGRESS', None, None), ('DELETE_COMPLETE', None, '/<old group>')]

Rows are (ResourceStatus, ResourceStatusReason, PhysicalResourceId) for the replaced logical id, oldest first, from DescribeStackEvents; the log-group names are per-run request values.

Expected (AWS), in order: ('UPDATE_IN_PROGRESS', 'Requested update requires the creation of a new physical resource; hence creating one.', <any>), ('UPDATE_COMPLETE', None, '/<new group>'), ('DELETE_SKIPPED', None, '/<old group>') and no DELETE_IN_PROGRESS/DELETE_COMPLETE for that logical id.
Actual (emulator): the CREATE_* triple above, then DELETE_IN_PROGRESS and DELETE_COMPLETE on the old group.

## Steps to Reproduce
1. Build and boot the emulator from this checkout (cloudformation and logs are both bootstrapped and serving).
2. CreateStack with one AWS::Logs::LogGroup (pinned LogGroupName, a RetentionInDays) declaring "UpdateReplacePolicy": "Retain"; put a log event into it.
3. UpdateStack with the same template but a different LogGroupName (create-only, so a replacement). Wait for UPDATE_COMPLETE.
4. DescribeStackEvents for that logical id: expect UPDATE_IN_PROGRESS with 'Requested update requires the creation of a new physical resource; hence creating one.' then UPDATE_COMPLETE with the new name; observe CREATE_IN_PROGRESS/CREATE_COMPLETE with no reason.
5. Expect one DELETE_SKIPPED carrying the OLD name and logs DescribeLogGroups/GetLogEvents still returning the old group and its event; observe DELETE_IN_PROGRESS + DELETE_COMPLETE and the group gone.
6. Equivalently: re-replay cloudformation__logs.cfn-logs-log-group-create-update-in-place-replace-retained-then-delete.
