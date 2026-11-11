Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam.cfn-iam-instance-profile-in-place-role-swap-retained-replacement-then-not-found
Issue-Key: cloudformation__iam.cfn-iam-instance-profile-in-place-role-swap-retained-replacement-then-not-found|_expect#25|AssertionError|raise AssertionError(
Issue-Commit: 94350cf775acbc496950503935930c48964de18f

# cloudformation__iam: an update that changes AWS::IAM::InstanceProfile properties fails UPDATE_FAILED with "NotImplemented: this emulator does not change the properties of an existing AWS::IAM::InstanceProfile in place" and rolls the stack back

> _Note: this issue was identified against the code base at commit `94350cf775acbc496950503935930c48964de18f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: the AWS::IAM::InstanceProfile resource provider implements no update path at all, so any stack update touching the resource fails the stack.
AWS: AWS::IAM::InstanceProfile supports updates. Changing the `Roles` list of an existing instance profile is an in-place (no-interruption) update — CloudFormation reports UPDATE_IN_PROGRESS then UPDATE_COMPLETE for the logical id on the SAME PhysicalResourceId, the profile keeps its name and ARN, and IAM's GetInstanceProfile afterwards reports the new role. Changing InstanceProfileName instead forces a replacement, which (with UpdateReplacePolicy: Retain) completes as UPDATE_COMPLETE on the new physical id and DELETE_SKIPPED on the old one. Either way the stack reaches UPDATE_COMPLETE.
Emulator: the update is refused by the resource provider itself. The logical id goes to `UPDATE_FAILED` with ResourceStatusReason `NotImplemented: this emulator does not change the properties of an existing AWS::IAM::InstanceProfile in place; the resource was left as it is.`, the stack ends `UPDATE_ROLLBACK_COMPLETE` with StackStatusReason `The following resource(s) failed to update: [Profile]. `, and the profile is left untouched.
Why: no stack containing an instance profile can ever be updated — the profile is the standard way a CloudFormation-managed EC2 instance or Auto Scaling group gets its role, so a routine role swap takes the whole stack into rollback instead of succeeding. A client also cannot distinguish "AWS refused this update" from "the emulator has no update path": the failure is reported as a resource-level provider error, not as any status AWS produces for a valid update.
Fix: the AWS::IAM::InstanceProfile resource provider's update handler in the CloudFormation provisioner — implement the in-place property update (at minimum the Roles list, via IAM RemoveRoleFromInstanceProfile/AddRoleToInstanceProfile on the existing profile) and classify name changes as replacement, instead of returning NotImplemented.
Traces: cloudformation__iam.cfn-iam-instance-profile-in-place-role-swap-retained-replacement-then-not-found.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1), `### cloudformation__iam` section (4/14 passed).

[mismatch] cloudformation__iam.cfn-iam-instance-profile-in-place-role-swap-retained-replacement-then-not-found
  AssertionError: stack ended 'UPDATE_ROLLBACK_COMPLETE' ('The following resource(s) failed to update: [Profile]. '), expected 'UPDATE_COMPLETE'; failed events: [('Profile', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::IAM::InstanceProfile in place; the resource was left as it is.')]

The check deploys a stack with an AWS::IAM::InstanceProfile (logical id Profile), updates it to swap the role it carries, and expects the stack to settle UPDATE_COMPLETE before going on to the replacement-and-retain leg.

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from the checkout.
2. CreateStack with an AWS::IAM::Role and an AWS::IAM::InstanceProfile whose Roles references it; wait for CREATE_COMPLETE.
3. UpdateStack with the profile's Roles pointing at a second role (name unchanged).
4. AWS reports UPDATE_IN_PROGRESS then UPDATE_COMPLETE on the same PhysicalResourceId and GetInstanceProfile shows the new role. The emulator reports Profile UPDATE_FAILED with 'NotImplemented: this emulator does not change the properties of an existing AWS::IAM::InstanceProfile in place; the resource was left as it is.' and the stack ends UPDATE_ROLLBACK_COMPLETE.
5. Replay cloudformation__iam.cfn-iam-instance-profile-in-place-role-swap-retained-replacement-then-not-found.
