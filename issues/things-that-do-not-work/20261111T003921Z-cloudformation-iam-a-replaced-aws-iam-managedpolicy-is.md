Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam.cfn-iam-managed-policy-create-update-in-place-replace-retained-then-delete
Issue-Key: cloudformation__iam.cfn-iam-managed-policy-create-update-in-place-replace-retained-then-delete|_assert_replaced_and_retained#30|AssertionError|assert _in_order(
Issue-Commit: b26d594b442f382e51363fa18dcb67bd5cc86e1e

# cloudformation__iam: a replaced AWS::IAM::ManagedPolicy is reported with CREATE_*/DELETE_* events and the retained old policy is deleted despite UpdateReplacePolicy Retain (2 causes)

> _Note: this issue was identified against the code base at commit `b26d594b442f382e51363fa18dcb67bd5cc86e1e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Two net-new root causes, both observed on one AWS::IAM::ManagedPolicy whose create-only ManagedPolicyName changes during an update. The failing check is marked neither ALREADY FILED nor OWNED.

Cause 1 of 2: the resource events of a replacement are reported as a CREATE, not as an UPDATE, and the replacement reason is missing.
AWS: Smithy (cloudformation) DescribeStackEvents -> StackEvent{ResourceStatus, ResourceStatusReason, PhysicalResourceId}, where ResourceStatus is an enum that includes UPDATE_IN_PROGRESS, UPDATE_COMPLETE and DELETE_SKIPPED. When an update changes a create-only property the resource is REPLACED inside that update: AWS emits UPDATE_IN_PROGRESS with ResourceStatusReason 'Requested update requires the creation of a new physical resource; hence creating one.' (fixed text), then UPDATE_IN_PROGRESS 'Resource creation Initiated', then UPDATE_COMPLETE carrying the NEW PhysicalResourceId (for a managed policy, its ARN - a value derived from the request and the account).
Emulator: the same update emits CREATE_IN_PROGRESS (ResourceStatusReason absent, PhysicalResourceId empty), CREATE_IN_PROGRESS 'Resource creation Initiated', CREATE_COMPLETE - no UPDATE_* event and no replacement reason.
Why: a client that follows DescribeStackEvents to decide what an update did cannot tell a replacement from a fresh create, and never sees the reason string that announces an in-use policy is about to be re-created under a new ARN.
Fix: the CloudFormation update engine's resource-event emission on the replacement path, not the IAM managed-policy provisioner.
Traces: cloudformation__iam.cfn-iam-managed-policy-create-update-in-place-replace-retained-then-delete.

Cause 2 of 2: UpdateReplacePolicy Retain is ignored - the replaced managed policy is deleted.
AWS: a resource whose UpdateReplacePolicy is Retain is NOT deleted in the cleanup phase of a replacement; CloudFormation emits DELETE_SKIPPED (a modeled ResourceStatus value) carrying the OLD PhysicalResourceId, and the old policy survives the update and the later DeleteStack - iam GetPolicy on the old ARN still answers.
Emulator: the cleanup phase emits DELETE_IN_PROGRESS (PhysicalResourceId absent) then DELETE_COMPLETE for the OLD ARN - no DELETE_SKIPPED, and the retained policy is destroyed.
Why: silent loss of a resource on a documented retention guarantee; worse for IAM, because principals outside the stack may still reference the retained policy ARN, and the event stream reports a successful delete rather than a skip, so the loss is undetectable from the stack's own record.
Fix: the CloudFormation update engine's cleanup phase - honour UpdateReplacePolicy (Retain/Snapshot/Delete) and emit DELETE_SKIPPED with the old physical id when it is Retain.
Traces: cloudformation__iam.cfn-iam-managed-policy-create-update-in-place-replace-retained-then-delete (the check fails at Cause 1; the event rows it prints also prove Cause 2).

The same engine-level behaviour is already recorded for AWS::SSM::Parameter in the open issue 20261110T232635Z-cloudformation-ssm-a-replacement-update-reports-create-delete-resource-events.md; its replay set names only the ssm check, so this recorded unit is not re-replayed by it.

## Evidence
Replay output (behavioral suite, section ### cloudformation__iam):
  [mismatch] cloudformation__iam.cfn-iam-managed-policy-create-update-in-place-replace-retained-then-delete
  AssertionError: the replacement must report creating a new resource, complete as 'arn:aws:iam::<account>:policy/<new name>' and skip deleting 'arn:aws:iam::<account>:policy/<old name>' (UpdateReplacePolicy Retain), in that order: [('CREATE_IN_PROGRESS', None, ''), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', 'arn:aws:iam::<account>:policy/<new name>'), ('CREATE_COMPLETE', None, 'arn:aws:iam::<account>:policy/<new name>'), ('DELETE_IN_PROGRESS', None, None), ('DELETE_COMPLETE', None, 'arn:aws:iam::<account>:policy/<old name>')]

Rows are (ResourceStatus, ResourceStatusReason, PhysicalResourceId) for the replaced logical id, oldest first, from DescribeStackEvents. The policy names/ARNs are per-run request and account values.

Expected (AWS), in order: ('UPDATE_IN_PROGRESS', 'Requested update requires the creation of a new physical resource; hence creating one.', <any>), ('UPDATE_COMPLETE', None, '<new policy ARN>'), ('DELETE_SKIPPED', None, '<old policy ARN>') and no DELETE_IN_PROGRESS/DELETE_COMPLETE for that logical id.
Actual (emulator): the CREATE_* triple above followed by DELETE_IN_PROGRESS and DELETE_COMPLETE on the old ARN.

## Steps to Reproduce
1. Build and boot the emulator from this checkout (cloudformation and iam are both bootstrapped and serving).
2. CreateStack with Capabilities=["CAPABILITY_NAMED_IAM"] and one AWS::IAM::ManagedPolicy (a pinned ManagedPolicyName, a PolicyDocument allowing s3:GetObject/s3:PutObject) declaring "UpdateReplacePolicy": "Retain".
3. UpdateStack with the same template but a different ManagedPolicyName (create-only, so a replacement). Wait for UPDATE_COMPLETE.
4. DescribeStackEvents for that logical id: expect UPDATE_IN_PROGRESS with 'Requested update requires the creation of a new physical resource; hence creating one.' then UPDATE_COMPLETE with the new ARN; observe CREATE_IN_PROGRESS/CREATE_COMPLETE with no reason.
5. Expect one DELETE_SKIPPED carrying the OLD ARN and iam GetPolicy(<old ARN>) still answering; observe DELETE_IN_PROGRESS + DELETE_COMPLETE and the old policy gone.
6. Equivalently: re-replay cloudformation__iam.cfn-iam-managed-policy-create-update-in-place-replace-retained-then-delete.
