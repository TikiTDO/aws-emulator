Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2.asghc-asg-round-trips-elb-health-check-grace-and-capacity-bounds
Issue-Key: autoscaling__ec2.asghc-asg-round-trips-elb-health-check-grace-and-capacity-bounds|run#14|KeyError|assert g["ServiceLinkedRoleARN"].endswith(
Issue-Commit: e985d084470b6245f57d747d28e3fbc5bb80c4f3

# autoscaling__ec2: DescribeAutoScalingGroups omits ServiceLinkedRoleARN, so a group created without one reports no service-linked role

> _Note: this issue was identified against the code base at commit `e985d084470b6245f57d747d28e3fbc5bb80c4f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAutoScalingGroups

## Description
One net-new root cause in the autoscaling__ec2 service heading.

Cause 1 of 1: DescribeAutoScalingGroups omits the ServiceLinkedRoleARN member from the AutoScalingGroup it returns for a group that was created without an explicit ServiceLinkedRoleARN.

AWS: Smithy (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01) AutoScalingGroup.ServiceLinkedRoleARN — "The Amazon Resource Name (ARN) of the service-linked role that the Auto Scaling group uses to call other Amazon Web Services on your behalf." CreateAutoScalingGroup.ServiceLinkedRoleARN is optional; when a caller omits it AWS binds the group to the account's default Auto Scaling service-linked role and DescribeAutoScalingGroups then always reports it. The value is an ARN whose variable parts are the partition, the account id and the role path/name taken from the caller's account and the service: arn:<partition>:iam::<account-id>:role/aws-service-role/autoscaling.amazonaws.com/AWSServiceRoleForAutoScaling. Only the trailing fixed segment ':role/aws-service-role/autoscaling.amazonaws.com/AWSServiceRoleForAutoScaling' is literal text; the account id is state, never a recorded literal. The member is non-optional in practice on the read path — the check asserts on its suffix, not on a whole recorded ARN.

Emulator: the DescribeAutoScalingGroups answer carries no ServiceLinkedRoleARN key at all. Replaying the check raises `KeyError: 'ServiceLinkedRoleARN'` at `assert g["ServiceLinkedRoleARN"].endswith(":role/aws-service-role/autoscaling.amazonaws.com/AWSServiceRoleForAutoScaling")`. Every other assertion in the same check passes first (HealthCheckType=ELB, HealthCheckGracePeriod=200, MinSize/MaxSize/DesiredCapacity=0/2/0, VPCZoneIdentifier, the launch-template id-to-name resolution, the derived AvailabilityZones, the service defaults DefaultCooldown=300 / TerminationPolicies=["Default"] / NewInstancesProtectedFromScaleIn=false / empty Instances, TargetGroupARNs, LoadBalancerNames, SuspendedProcesses, the absence of unset structures, and the propagate-at-launch tag row), so the omitted role ARN is the single divergence this check exposes.

Why: a client that reads the group to learn which principal Auto Scaling acts as — a replace-loop or launch-failure diagnosis, a drift check, or any code that re-submits the described group through CreateAutoScalingGroup/UpdateAutoScalingGroup — gets a KeyError or silently loses the role binding. Round-tripping a described group therefore cannot reproduce the group AWS would create.

Fix: the DescribeAutoScalingGroups serializer and the group state that backs it (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01) — CreateAutoScalingGroup must default the group's service-linked role to the account's AWSServiceRoleForAutoScaling ARN when the request omits ServiceLinkedRoleARN, and the describe path must always emit that member.

Related open issues describe the same omission reached through other service headings and are NOT marked as owning this check: 20261107T075601Z-autoscaling-ec2-kms-auto-scaling-group-reads-omit-servicelinkedrolearn-and-group.md, 20261109T164957Z-autoscaling-ec2-events-kms-sqs-describeautoscalinggroups-omits-servicelinkedrolearn.md and 20261112T161208Z-autoscaling-ec2-kms-a-group-created-without-a.md. A fixer closing this issue should check whether the same fix closes those.

Traces: autoscaling__ec2.asghc-asg-round-trips-elb-health-check-grace-and-capacity-bounds (verdict: error).

## Evidence
Failing check: autoscaling__ec2.asghc-asg-round-trips-elb-health-check-grace-and-capacity-bounds [error]

Expected (AWS): DescribeAutoScalingGroups returns the group with a ServiceLinkedRoleARN member ending in ':role/aws-service-role/autoscaling.amazonaws.com/AWSServiceRoleForAutoScaling' (account id and partition taken from the caller's account), because CreateAutoScalingGroup was called without ServiceLinkedRoleARN and AWS binds the default Auto Scaling service-linked role.

Actual (emulator): the member is absent from the response. Replay trace:
  File "check_autoscaling__ec2_asghc_asg_round_trips_elb_health_check_grace_and_capacity_bounds.py", line 97, in run
    assert g["ServiceLinkedRoleARN"].endswith(
  KeyError: 'ServiceLinkedRoleARN'

All preceding assertions in the check (round-tripped HealthCheckType/HealthCheckGracePeriod/MinSize/MaxSize/DesiredCapacity/VPCZoneIdentifier, resolved LaunchTemplate name, derived AvailabilityZones, service defaults, absent unset structures) passed, isolating the omitted member as the cause.

## Steps to Reproduce
1. Create a VPC with one subnet in an AZ that offers t3.micro, a security group and an EC2 launch template.
2. Call autoscaling CreateAutoScalingGroup referencing the launch template BY ID with Version="$Latest", HealthCheckType="ELB", HealthCheckGracePeriod=200, MinSize=0, MaxSize=2, DesiredCapacity=0, VPCZoneIdentifier=<subnet>, one tag with PropagateAtLaunch=true, and NO ServiceLinkedRoleARN.
3. Call autoscaling DescribeAutoScalingGroups for that group name.
4. Observe the returned AutoScalingGroup has no ServiceLinkedRoleARN key. AWS returns one ending in ':role/aws-service-role/autoscaling.amazonaws.com/AWSServiceRoleForAutoScaling'.

Replay key: behavioral check autoscaling__ec2.asghc-asg-round-trips-elb-health-check-grace-and-capacity-bounds
