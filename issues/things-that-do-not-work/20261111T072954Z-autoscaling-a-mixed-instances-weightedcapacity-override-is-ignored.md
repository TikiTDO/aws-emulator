Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.set-desired-capacity-counts-weighted-units-not-instances
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling.set-desired-capacity-counts-weighted-units-not-instances|run#25|AssertionError|assert got == [("t3.micro", "2")], (
Issue-Commit: f456950adb04cb9e7696776dd0db02907fd8875a

# autoscaling: a mixed-instances WeightedCapacity override is ignored — DesiredCapacity is met in instances, not capacity units, and Instances[].WeightedCapacity is never serialized

> _Note: this issue was identified against the code base at commit `f456950adb04cb9e7696776dd0db02907fd8875a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetDesiredCapacity

## Description
One net-new root cause in the Auto Scaling capacity-unit accounting.

Cause 1 of 1: a group with a MixedInstancesPolicy whose LaunchTemplate.Overrides carry WeightedCapacity is still scaled as if every instance counted as one unit, and the weight is absent from every read of the group's members. Failing check id: autoscaling.set-desired-capacity-counts-weighted-units-not-instances.
  AWS: Smithy (autoscaling): LaunchTemplateOverrides.WeightedCapacity (XmlStringMaxLen32, range 1-999) documents that "you can specify the number of capacity units provided by each instance type... the capacity units count toward the desired capacity. Amazon EC2 Auto Scaling launches instances until the desired capacity is totally fulfilled", and that when a mixed instances policy defines weights, DesiredCapacity/MinSize/MaxSize are expressed in those same units. So for a single override of InstanceType=t3.micro with WeightedCapacity="2", SetDesiredCapacity(DesiredCapacity=2) is satisfied by exactly ONE instance. Smithy also models the weight on both read paths: Instance.WeightedCapacity (DescribeAutoScalingGroups -> AutoScalingGroups[].Instances[]) and AutoScalingInstanceDetails.WeightedCapacity (DescribeAutoScalingInstances), each carrying the override's value as a string ("2" here). SetDesiredCapacity itself models only ScalingActivityInProgress/ResourceContention, so this is state behavior, not a refusal.
  Emulator: SetDesiredCapacity(2) on that group launches TWO instances, and neither read carries the weight — `AssertionError: group instances (type, weighted capacity) for 2 units at weight 2: expected [('t3.micro', '2')], got [('t3.micro', None), ('t3.micro', None)]`. The member list reaches two InService instances with WeightedCapacity absent from the serialized Instance (botocore yields no key, hence None), so DesiredCapacity is being interpreted as an instance count and the override's weight is dropped at both the planner and the serializer.
  Why: a client that weights instance types sizes its group in capacity units; the emulator over-provisions by the weight factor (here 2x the fleet, 2x the cost) for every scale-out, and a client that reads Instances[].WeightedCapacity or DescribeAutoScalingInstances[].WeightedCapacity to reconcile units against members sees nothing to reconcile with. Capacity-unit accounting — including any downstream weighted metric — is unusable.
  Fix: the Auto Scaling group's capacity planner (how many instances the group launches to fulfil DesiredCapacity when the group has a MixedInstancesPolicy with weighted overrides) plus the per-instance weight it records in group state, and the DescribeAutoScalingGroups / DescribeAutoScalingInstances serializers that must emit WeightedCapacity for a weighted member.
  Traces: autoscaling.set-desired-capacity-counts-weighted-units-not-instances

Related open issues that do NOT carry this replay key and describe a different seam: 20261109T172356Z-autoscaling-cloudwatch-ec2-a-weighted-capacity-auto-scaling.md and 20261110T160612Z-autoscaling-cloudwatch-ec2-a-weighted-capacity-auto-scaling-cb74ea.md (weighted group metrics at CloudWatch, service autoscaling__cloudwatch__ec2).

## Evidence
Expected: after create_auto_scaling_group(MinSize=0, MaxSize=4, DesiredCapacity=0, MixedInstancesPolicy={LaunchTemplate:{LaunchTemplateSpecification:{...}, Overrides:[{InstanceType:'t3.micro', WeightedCapacity:'2'}]}, InstancesDistribution:{...}}) and set_desired_capacity(DesiredCapacity=2), DescribeAutoScalingGroups reports exactly ONE InService member whose (InstanceType, WeightedCapacity) is ('t3.micro', '2'), DesiredCapacity reads 2 (units), and DescribeAutoScalingInstances reports (group, '2') for that member.
Actual: `AssertionError: group instances (type, weighted capacity) for 2 units at weight 2: expected [('t3.micro', '2')], got [('t3.micro', None), ('t3.micro', None)]` — two instances launched and WeightedCapacity absent from both. The instance ids and group name in any one run are per-run values, not part of the requirement; the counts and the weight string are.

## Steps to Reproduce
1. CreateLaunchTemplate with any AL2023 ImageId and InstanceType t3.micro.
2. CreateAutoScalingGroup(MinSize=0, MaxSize=4, DesiredCapacity=0, VPCZoneIdentifier=<subnet>, MixedInstancesPolicy={LaunchTemplate:{LaunchTemplateSpecification:{LaunchTemplateName:<lt>, Version:'$Latest'}, Overrides:[{InstanceType:'t3.micro', WeightedCapacity:'2'}]}, InstancesDistribution:{OnDemandBaseCapacity:0, OnDemandPercentageAboveBaseCapacity:100}}).
3. SetDesiredCapacity(AutoScalingGroupName=<group>, DesiredCapacity=2) and wait for all activities to settle.
4. DescribeAutoScalingGroups(AutoScalingGroupNames=[<group>]) — observe two Instances entries instead of one, each with no WeightedCapacity; DescribeAutoScalingInstances on them likewise reports no WeightedCapacity.
5. Replay autoscaling.set-desired-capacity-counts-weighted-units-not-instances.
