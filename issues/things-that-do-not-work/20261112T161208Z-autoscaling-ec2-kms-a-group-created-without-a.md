Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__kms.asglf-group-round-trips-bounds-template-reference-and-defaults
Issue-Key: autoscaling__ec2__kms.asglf-group-round-trips-bounds-template-reference-and-defaults|run#13|AssertionError|assert slr == (f"arn:aws:iam::{ctx.account}:role/aws-service-role/"
Issue-Commit: 7394251eefd071b8c239bc91125495753c22c75a

# autoscaling__ec2__kms: a group created without a ServiceLinkedRoleARN reports no service-linked role, so DescribeAutoScalingGroups yields an empty/absent ServiceLinkedRoleARN (1 cause)

> _Note: this issue was identified against the code base at commit `7394251eefd071b8c239bc91125495753c22c75a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAutoScalingGroups

## Description
One net-new root cause behind the surviving autoscaling__ec2__kms failure. NOTE: two open issues describe the same member — 20261107T075601Z-autoscaling-ec2-kms-auto-scaling-group-reads-omit-servicelinkedrolearn-and-group.md (its Cause 1) and 20261109T164957Z-autoscaling-ec2-events-kms-sqs-describeautoscalinggroups-omits-servicelinkedrolearn.md — but neither carries this check's current replay key: the check now gets past the declared/derived round-trip assertion those issues recorded (`assert declared == {`) and fails further on, at the service-linked-role assertion. It is therefore filed here as its own work item and those issues are named for context.

Cause 1 of 1: DescribeAutoScalingGroups does not report the Auto Scaling group's service-linked role for a group created without ServiceLinkedRoleARN.
AWS: the Smithy autoscaling AutoScalingGroup shape carries member ServiceLinkedRoleARN (target ResourceName) — "The Amazon Resource Name (ARN) of the service-linked role that the Auto Scaling group uses to call other Amazon Web Services on your behalf". CreateAutoScalingGroup may omit it; AWS then binds the group to the account's default Auto Scaling service-linked role and every subsequent read reports that ARN. Its shape is fixed text with one variable part — the caller's account id — in the form arn:aws:iam::<account-id>:role/aws-service-role/autoscaling.amazonaws.com/AWSServiceRoleForAutoScaling (the role path and role name are the fixed part; only the account id comes from the caller). No error is involved: the call succeeds with HTTP 200 in both AWS and the emulator, so the divergence is purely in the response body.
Emulator: for a group created with MinSize=0/MaxSize=1/DesiredCapacity=0 from a launch template by id, VPCZoneIdentifier=<subnet>, and no ServiceLinkedRoleARN in the request, the described group's ServiceLinkedRoleARN reads back as the empty string (the member is either absent or serialized empty) — the replay reports `AssertionError: the group's service-linked role is ''`.
Why: a client that reads the group's role to act on it cannot proceed. Everything downstream of that read is unreachable: granting that role kms:CreateGrant/Decrypt on the CMK that encrypts the launch template's root volume, diagnosing an encrypted-volume launch failure, or writing an IAM/KMS policy whose principal is the group's role. A caller indexing the member outright (group["ServiceLinkedRoleARN"]) raises KeyError; one using a default silently builds a policy with an empty principal.
Fix: the AutoScalingGroup representation — CreateAutoScalingGroup should default the group's service-linked role to the account's AWSServiceRoleForAutoScaling ARN when the request omits one, store it, and the DescribeAutoScalingGroups serializer should always emit ServiceLinkedRoleARN for every group it returns.
Traces: autoscaling__ec2__kms.asglf-group-round-trips-bounds-template-reference-and-defaults.

## Evidence
From the behavioral replay of autoscaling__ec2__kms.asglf-group-round-trips-bounds-template-reference-and-defaults [mismatch]:

  File "check_autoscaling__ec2__kms_asglf_group_round_trips_bounds_template_reference_and_defaults.py", line 47, in run
    assert slr == (f"arn:aws:iam::{ctx.account}:role/aws-service-role/"
                   f"autoscaling.amazonaws.com/AWSServiceRoleForAutoScaling")
  AssertionError: the group's service-linked role is ''

Expected (AWS): DescribeAutoScalingGroups returns the group with
  ServiceLinkedRoleARN = arn:aws:iam::<account-id>:role/aws-service-role/autoscaling.amazonaws.com/AWSServiceRoleForAutoScaling
where only <account-id> varies with the caller; the rest of the ARN is fixed text.

Actual (emulator): the same read yields '' for ServiceLinkedRoleARN (member absent or empty). The declared/derived assertions earlier in the same check (MinSize/MaxSize/DesiredCapacity/VPCZoneIdentifier/LaunchTemplate/AvailabilityZones and the documented defaults) now pass, so the service-linked role is the only remaining divergence in this check.

## Steps to Reproduce
1. Replay the behavioral suite and re-run autoscaling__ec2__kms.asglf-group-round-trips-bounds-template-reference-and-defaults.
2. Equivalently, by hand: create a VPC with one subnet in an AZ offering t3.micro; create a KMS symmetric key; ec2:CreateLaunchTemplate with LaunchTemplateData carrying an encrypted gp3 root BlockDeviceMapping under that key.
3. autoscaling:CreateAutoScalingGroup with MinSize=0, MaxSize=1, DesiredCapacity=0, LaunchTemplate={LaunchTemplateId:<id>, Version:"1"}, VPCZoneIdentifier=<subnet>, and NO ServiceLinkedRoleARN.
4. autoscaling:DescribeAutoScalingGroups(AutoScalingGroupNames=[<name>]) and read AutoScalingGroups[0].ServiceLinkedRoleARN.
5. Observe it is absent/empty; AWS reports arn:aws:iam::<account-id>:role/aws-service-role/autoscaling.amazonaws.com/AWSServiceRoleForAutoScaling.
