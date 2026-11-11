Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudformation__ec2.cfn-autoscaling-group-in-place-max-size-retained-replacement-enforces-bounds-then-not-found
Issue-Key: autoscaling__cloudformation__ec2.cfn-autoscaling-group-in-place-max-size-retained-replacement-enforces-bounds-then-not-found|<genexpr>#24|KeyError|(t["ResourceId"], t["ResourceType"], t["Key"], t["Value"], t["PropagateAtLaunch"])
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# Auto Scaling tag entries omit PropagateAtLaunch

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTags

## Description
Cause 1 of 1: the tags Auto Scaling reports for a CloudFormation-created group have no PropagateAtLaunch member.

AWS: an Auto Scaling TagDescription always carries ResourceId, ResourceType, Key, Value and PropagateAtLaunch; PropagateAtLaunch is the flag the AWS::AutoScaling::AutoScalingGroup Tags entries set and defaults to false when the group was created without it. It is returned by DescribeTags and inside DescribeAutoScalingGroups' Tags list.

Emulator: the tag entries carry ResourceId, ResourceType, Key and Value but not PropagateAtLaunch, so a client indexing t['PropagateAtLaunch'] raises KeyError: 'PropagateAtLaunch'.

Why: PropagateAtLaunch is how a caller tells whether a group tag will be stamped onto launched instances; a client that reads it (as the modelled shape permits) crashes, and one that defaults it silently mis-reports instance tagging.

Fix: the Auto Scaling tag state / DescribeTags and DescribeAutoScalingGroups serializers — persist the per-tag PropagateAtLaunch flag (default false) and always emit it.

Traces: the failing check in trace_ids.

## Evidence
Expected: every tag description exposes (ResourceId, ResourceType, Key, Value, PropagateAtLaunch).
Actual: KeyError: 'PropagateAtLaunch' raised while reading the group's tags at `(t["ResourceId"], t["ResourceType"], t["Key"], t["Value"], t["PropagateAtLaunch"])`.

## Steps to Reproduce
1. CreateStack an AWS::AutoScaling::AutoScalingGroup with a Tags entry (Key/Value/PropagateAtLaunch).
2. Call autoscaling DescribeTags (or DescribeAutoScalingGroups) for that group.
3. The returned tag entries have no PropagateAtLaunch member.
