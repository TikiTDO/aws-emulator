Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda.cfn-lambda-function-in-place-update-retained-replacement-then-not-found
Issue-Key: cloudformation__lambda.cfn-lambda-function-in-place-update-retained-replacement-then-not-found|_assert_replaced_and_retained#30|AssertionError|assert (
Issue-Commit: b26d594b442f382e51363fa18dcb67bd5cc86e1e

# cloudformation__lambda: a replaced AWS::Lambda::Function is reported with CREATE_*/DELETE_* events and the retained old function is deleted despite UpdateReplacePolicy Retain (2 causes)

> _Note: this issue was identified against the code base at commit `b26d594b442f382e51363fa18dcb67bd5cc86e1e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Two net-new root causes, both observed on one AWS::Lambda::Function whose create-only FunctionName changes during an update. The failing check is marked neither ALREADY FILED nor OWNED.

Cause 1 of 2: a replacement is not announced - its resource events are a CREATE, not an UPDATE, and the replacement reason is missing.
AWS: Smithy (cloudformation) DescribeStackEvents -> StackEvent{ResourceStatus, ResourceStatusReason, PhysicalResourceId}; ResourceStatus is an enum that includes UPDATE_IN_PROGRESS, UPDATE_COMPLETE and DELETE_SKIPPED. When an update changes a create-only property the resource is REPLACED inside the update: AWS emits UPDATE_IN_PROGRESS with ResourceStatusReason 'Requested update requires the creation of a new physical resource; hence creating one.' (fixed text), then UPDATE_IN_PROGRESS 'Resource creation Initiated', then UPDATE_COMPLETE carrying the NEW PhysicalResourceId (here the new function name, a request value).
Emulator: CREATE_IN_PROGRESS (reason absent, PhysicalResourceId empty), CREATE_IN_PROGRESS 'Resource creation Initiated', CREATE_COMPLETE - no UPDATE_* event and no replacement reason, so the check's 'Fn must announce its replacement' assertion fails.
Why: a client that follows the event stream to see what an update did (console progress, CI gates, drift/audit tooling) cannot tell a replaced function from a newly created one, and never sees the reason that warns an invoked function is about to be re-created under a new name/ARN.
Fix: the CloudFormation update engine's resource-event emission on the replacement path, not the Lambda function provisioner.
Traces: cloudformation__lambda.cfn-lambda-function-in-place-update-retained-replacement-then-not-found.

Cause 2 of 2: UpdateReplacePolicy Retain is ignored - the replaced function is deleted.
AWS: with UpdateReplacePolicy Retain the old physical resource is NOT deleted in the replacement's cleanup phase; CloudFormation emits DELETE_SKIPPED (a modeled ResourceStatus value) with the OLD PhysicalResourceId, and the old function survives the update and the later DeleteStack - lambda GetFunction on the old name still answers.
Emulator: DELETE_IN_PROGRESS (PhysicalResourceId absent) then DELETE_COMPLETE for the OLD function name - no DELETE_SKIPPED, and the retained function is destroyed.
Why: silent loss of a resource on a documented retention guarantee; event sources, aliases and callers pinned to the retained function break, and the event stream reports a successful delete rather than a skip, so the loss is undetectable from the stack's record.
Fix: the CloudFormation update engine's cleanup phase - honour UpdateReplacePolicy (Retain/Snapshot/Delete) and emit DELETE_SKIPPED with the old physical id when it is Retain.
Traces: cloudformation__lambda.cfn-lambda-function-in-place-update-retained-replacement-then-not-found (the check fails at Cause 1; its printed event rows also prove Cause 2).

The same engine-level behaviour is already recorded for AWS::SSM::Parameter in the open issue 20261110T232635Z-cloudformation-ssm-a-replacement-update-reports-create-delete-resource-events.md; its replay set names only the ssm check, so this recorded unit is not re-replayed by it.

## Evidence
Replay output (behavioral suite, section ### cloudformation__lambda):
  [mismatch] cloudformation__lambda.cfn-lambda-function-in-place-update-retained-replacement-then-not-found
  AssertionError: Fn must announce its replacement, its events were [('CREATE_IN_PROGRESS', None, ''), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', '<new function name>'), ('CREATE_COMPLETE', None, '<new function name>'), ('DELETE_IN_PROGRESS', None, None), ('DELETE_COMPLETE', None, '<old function name>')]

Rows are (ResourceStatus, ResourceStatusReason, PhysicalResourceId) for the replaced logical id, oldest first, from DescribeStackEvents; the function names are per-run request values.

Expected (AWS), in order: ('UPDATE_IN_PROGRESS', 'Requested update requires the creation of a new physical resource; hence creating one.', <any>), ('UPDATE_COMPLETE', None, '<new function name>'), ('DELETE_SKIPPED', None, '<old function name>') and no DELETE_IN_PROGRESS/DELETE_COMPLETE for that logical id.
Actual (emulator): the CREATE_* triple above, then DELETE_IN_PROGRESS and DELETE_COMPLETE on the old function.

## Steps to Reproduce
1. Build and boot the emulator from this checkout (cloudformation and lambda are both bootstrapped and serving).
2. CreateStack with one AWS::Lambda::Function (pinned FunctionName, inline/zip code, a role) declaring "UpdateReplacePolicy": "Retain"; wait for CREATE_COMPLETE.
3. UpdateStack with the same template but a different FunctionName (create-only, so a replacement). Wait for UPDATE_COMPLETE.
4. DescribeStackEvents for that logical id: expect UPDATE_IN_PROGRESS with 'Requested update requires the creation of a new physical resource; hence creating one.' then UPDATE_COMPLETE with the new name; observe CREATE_IN_PROGRESS/CREATE_COMPLETE with no reason.
5. Expect one DELETE_SKIPPED carrying the OLD function name and lambda GetFunction(<old>) still answering; observe DELETE_IN_PROGRESS + DELETE_COMPLETE and the old function gone.
6. Equivalently: re-replay cloudformation__lambda.cfn-lambda-function-in-place-update-retained-replacement-then-not-found.
