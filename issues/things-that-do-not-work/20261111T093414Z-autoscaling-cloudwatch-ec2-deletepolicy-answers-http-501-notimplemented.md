Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudwatch__ec2.target-tracking-disable-scale-in-omits-the-scale-in-alarm
Issue-Key: autoscaling__cloudwatch__ec2|DeletePolicy|NotImplemented
Issue-Commit: c3de02674d16064d9f5c8a9be8af17d3d848d40e

# autoscaling__cloudwatch__ec2: DeletePolicy answers HTTP 501 NotImplemented, so a target-tracking policy's alarms can never be scoped-deleted (1 cause)

> _Note: this issue was identified against the code base at commit `c3de02674d16064d9f5c8a9be8af17d3d848d40e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeletePolicy

## Description
Cause 1 of 1: autoscaling:DeletePolicy is an unimplemented scaffold stub and answers HTTP 501 with error code NotImplemented, so no scaling policy can ever be deleted and the CloudWatch alarms a target-tracking policy minted can never be reclaimed.

AWS: Smithy models DeletePolicy with input AutoScalingGroupName (XmlStringMaxLen255, len 1..255) and PolicyName (ResourceName, len 1..1600) @required, an empty output (Unit), and exactly two modeled errors — ResourceContentionFault (HTTP 500, awsQuery code ResourceContention) and ServiceLinkedRoleFailure (HTTP 500). NotImplemented is not among them. The operation's own documentation states: 'Deletes the specified scaling policy. Deleting either a step scaling policy or a simple scaling policy deletes the underlying alarm action, but does not delete the alarm, even if it no longer has an associated action.' For a TargetTrackingScaling policy, deleting the policy deletes the alarms Auto Scaling itself created for it. So the success response is HTTP 200 with an empty body; the only fixed-text refusals are the two 500 faults above, whose messages are the model's documented sentences.

Emulator: the call is refused — 'An error occurred (NotImplemented) when calling the DeletePolicy operation: operation DeletePolicy is scaffolded but not implemented in this emulator'. The handler at services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/delete_policy.rs:177 returns crate::scaffolded::Scaffolded::not_implemented("DeletePolicy") and the file still carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker. In autoscaling__cloudwatch__ec2.target-tracking-disable-scale-in-omits-the-scale-in-alarm the check puts two TargetTrackingScaling policies (one with DisableScaleIn=False, one with DisableScaleIn=True, both PredefinedMetricType=ASGAverageCPUUtilization, TargetValue=43.5) and then asserts that deleting the flagged policy removes exactly ITS CloudWatch alarms and leaves the twin's; the DeletePolicy call errors the check out instead.

Why: a client cannot tear down or replace a scaling policy at all — every delete is a hard 501 rather than a 200, so policy churn (CDK/CloudFormation replacement, blue/green reconfiguration) fails, and the alarms a target-tracking policy created leak forever in CloudWatch. Because 501 is not a modeled error, SDK retry/error handling classifies it as a server fault rather than an actionable condition.

Fix: implement the operation's handler in services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/delete_policy.rs — remove the named policy from the group's policy collection in the autoscaling service state, delete the CloudWatch alarms Auto Scaling minted for a TargetTrackingScaling policy (and only the alarm ACTION for step/simple policies), answer HTTP 200 with the empty output, and drop the SCAFFOLDED marker. The equivalent stub is already filed for the plain `autoscaling` service as 20261111T072954Z-autoscaling-deletepolicy-stub.md; that issue does not carry this check's replay key, so it does not cover this one.

Traces: autoscaling__cloudwatch__ec2.target-tracking-disable-scale-in-omits-the-scale-in-alarm.

## Evidence
Expected (AWS): DeletePolicy(AutoScalingGroupName=<g>, PolicyName=<p>) -> HTTP 200, empty body; the TargetTrackingScaling policy's generated CloudWatch alarms disappear and the sibling policy's alarms remain. Only ResourceContentionFault (500) and ServiceLinkedRoleFailure (500) are modeled refusals.

Actual (emulator): HTTP 501, Error.Code='NotImplemented', Error.Message='operation DeletePolicy is scaffolded but not implemented in this emulator' — the fixed text is emulator-authored and names the operation; AWS has no such code or sentence for this operation.

## Steps to Reproduce
1. Create a launch template and an Auto Scaling group (MinSize=0, MaxSize=4, DesiredCapacity=0).
2. autoscaling:PutScalingPolicy(AutoScalingGroupName=<g>, PolicyName=<p>, PolicyType='TargetTrackingScaling', TargetTrackingConfiguration={PredefinedMetricSpecification:{PredefinedMetricType:'ASGAverageCPUUtilization'}, TargetValue:43.5, DisableScaleIn:true}).
3. autoscaling:DeletePolicy(AutoScalingGroupName=<g>, PolicyName=<p>).
4. Observe HTTP 501 NotImplemented instead of HTTP 200 with an empty body.
