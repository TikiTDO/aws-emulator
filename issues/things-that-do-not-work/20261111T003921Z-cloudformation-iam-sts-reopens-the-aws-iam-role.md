Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__sts.cfn-iam-role-create-update-in-place-replace-retained-then-delete
Issue-Key: cloudformation__iam__sts.cfn-iam-role-create-update-in-place-replace-retained-then-delete|_update#26|AssertionError|assert stack["StackStatus"] == "UPDATE_COMPLETE", (
Issue-Commit: b26d594b442f382e51363fa18dcb67bd5cc86e1e

# cloudformation__iam__sts: REOPENS the AWS::IAM::Role in-place update — a stack update of an existing role is still refused with NotImplemented and rolls back

> _Note: this issue was identified against the code base at commit `b26d594b442f382e51363fa18dcb67bd5cc86e1e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
REOPENS 20261025T121312Z-cloudformation-a-stack-update-that-changes-an-aws.md: its fix left the call refused.

One root cause (Cause 1 of 1), grounded in a failing behavioral check the replay report marks neither ALREADY FILED nor OWNED. That resolved issue removed the refusal of an in-place AWS::IAM::Role update (a changed AssumeRolePolicyDocument), but a stack update that changes an existing role's in-place properties is still refused with the same emulator prose, now on a different recorded unit.

Cause 1 of 1: a stack update that changes only no-interruption properties of an existing AWS::IAM::Role ends UPDATE_ROLLBACK_COMPLETE instead of UPDATE_COMPLETE, and nothing is applied to the role.
AWS: AssumeRolePolicyDocument, Description, MaxSessionDuration, Policies, ManagedPolicyArns and Tags are no-interruption (in-place) update properties of AWS::IAM::Role. CloudFormation applies them through IAM's own calls (Smithy iam#UpdateAssumeRolePolicy - required RoleName and PolicyDocument, modeled errors NoSuchEntity 404 / MalformedPolicyDocument 400 / UnmodifiableEntity 400 / LimitExceeded 409 / ServiceFailure 500 - plus UpdateRole, PutRolePolicy/DeleteRolePolicy, AttachRolePolicy/DetachRolePolicy, TagRole/UntagRole). The role keeps its identity (same RoleName, RoleId and PhysicalResourceId) and DescribeStacks reports UPDATE_COMPLETE. Only a changed RoleName is a replacement. AWS emits no refusal on this path.
Emulator: the update ends UPDATE_ROLLBACK_COMPLETE with stack reason 'The following resource(s) failed to update: [<LogicalResourceId>]. ' and the resource reason 'NotImplemented: this emulator does not change the properties of an existing AWS::IAM::Role in place; the resource was left as it is.' The fixed text of that reason is invented prose and 'NotImplemented' is not an AWS error code; the variable parts are the logical id and the resource type. The role is left unchanged.
Why: a client still cannot evolve a role through its template - the single most common IAM edit a stack performs. The stack is left rolled back, so the template and the deployed estate diverge and a retrying pipeline never converges; and because the previous fix was closed, this path is now believed to work.
Fix: the AWS::IAM::Role arm of the CloudFormation per-type provisioner's update path (services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioner.rs, Provisioner::update; its `_ => Ok(None)` fallthrough is what services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioning.rs `reconcile` turns into this reason) must cover every in-place property this check changes, not only the one the earlier fix addressed, and a changed RoleName must keep going down the replacement path.
Traces: cloudformation__iam__sts.cfn-iam-role-create-update-in-place-replace-retained-then-delete.

## Evidence
Replay output (behavioral suite, section ### cloudformation__iam__sts):
  [mismatch] cloudformation__iam__sts.cfn-iam-role-create-update-in-place-replace-retained-then-delete
  AssertionError: update ended 'UPDATE_ROLLBACK_COMPLETE': 'The following resource(s) failed to update: [Role]. '; [('Role', 'NotImplemented: this emulator does not change the properties of an existing AWS::IAM::Role in place; the resource was left as it is.')]

Expected (AWS): StackStatus UPDATE_COMPLETE, the role's PhysicalResourceId and RoleId unchanged, iam GetRole reporting the new in-place property values.
Actual (emulator): StackStatus UPDATE_ROLLBACK_COMPLETE with the quoted resource reason. 'Role' is the template's logical id (a request value); the remaining text is fixed emulator prose AWS never emits.
The resolved issue 20261025T121312Z-cloudformation-a-stack-update-that-changes-an-aws.md quotes the identical reason string for the same resource type, so this is that refusal still standing, not a new defect.

## Steps to Reproduce
1. Build and boot the emulator from this checkout (cloudformation, iam and sts are all bootstrapped and serving).
2. CreateStack with Capabilities=["CAPABILITY_NAMED_IAM"] and one AWS::IAM::Role (pinned RoleName, an AssumeRolePolicyDocument trusting the account root), declaring "UpdateReplacePolicy": "Retain". Wait for CREATE_COMPLETE.
3. UpdateStack with the same template and Capabilities, changing only in-place properties of the role (e.g. the trust policy's principal and/or Description), leaving RoleName alone.
4. Poll DescribeStacks. Expect UPDATE_COMPLETE with iam GetRole reporting the new values and the same RoleId. Observe UPDATE_ROLLBACK_COMPLETE and the resource reason 'NotImplemented: this emulator does not change the properties of an existing AWS::IAM::Role in place; the resource was left as it is.'
5. Equivalently: re-replay cloudformation__iam__sts.cfn-iam-role-create-update-in-place-replace-retained-then-delete.
