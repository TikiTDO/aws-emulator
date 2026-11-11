Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.asg-target-group-mismatched-vpc-fails-registration
Issue-Key: autoscaling__ecs__elbv2.asg-target-group-mismatched-vpc-fails-registration|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Commit: dff6e291e5e6489707888b0049fa49b367909ca3

# ASG launch into a target group in a mismatched VPC is never cancelled: no 'Failed to update target group' scaling activity is ever recorded

> _Note: this issue was identified against the code base at commit `dff6e291e5e6489707888b0049fa49b367909ca3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeScalingActivities

## Description
Cause 1 of 1: an Auto Scaling group attached to an ELBv2 target group that belongs to a DIFFERENT VPC than the group's subnets never produces the cancelled launch AWS produces, so the check polling for it cannot reach a graded state and the whole check exhausts its 60 s budget.

AWS: recorded behavior only (the autoscaling Smithy model exposes DescribeScalingActivities -> Activity.StatusCode/StatusMessage but states no cross-VPC rule). The recording shows the instance launch being CANCELLED, with Activity.StatusCode = "Cancelled" and StatusMessage containing the fixed text "Failed to update target group " followed by the target group ARN from the request, and DescribeTargetHealth on that mismatched group returning an empty TargetHealthDescriptions list. After detaching the mismatched group and attaching one in the matching VPC, the next instance IS registered.

Emulator: no such activity ever appears. DescribeScalingActivities returns no Cancelled activity whose StatusMessage mentions the target group, so the poll never terminates and the check is killed at its 60 s budget ("Error: Check timed out after 60.0s"). The emulator accepts the cross-VPC attachment silently and records no failure on the launch path.

Why: a client validating that a misconfigured target group (wrong VPC) is surfaced as a cancelled launch sees an ASG that looks healthy and an empty target group with no explanation. The negative control AWS gives operators — the cancelled activity naming the group it could not update — is absent, so misconfiguration is undiagnosable against the emulator and automation that waits for that activity hangs.

Fix: the autoscaling service's instance-launch/registration path and its scaling-activity recorder — registering a newly launched instance with each attached target group must validate the group's VpcId against the instance's subnet VPC, cancel the launch on mismatch, and append an Activity with StatusCode "Cancelled" and the "Failed to update target group <targetGroupArn>" message. The 60 s budget exhaustion is a consequence, not a separate defect.

Traces: autoscaling__ecs__elbv2.asg-target-group-mismatched-vpc-fails-registration. Neighbouring open issues for this service (20261107T080429Z-autoscaling-ecs-elbv2-6-net-new-causes-asg.md, 20261110T211308Z-autoscaling-ecs-elbv2-2-net-new-causes-registertargets.md) do not carry this check's replay key.

## Evidence
Expected (AWS): DescribeScalingActivities contains an Activity with StatusCode='Cancelled' and StatusMessage containing the fixed text 'Failed to update target group ' + the mismatched target group ARN; DescribeTargetHealth on that group returns [].
Actual (emulator): no Cancelled activity with that message is ever returned; the poll runs until the harness kills the check — 'Error: Check timed out after 60.0s'.

## Steps to Reproduce
1. Create VPC A with subnets and VPC B.
2. CreateTargetGroup in VPC B (the mismatched one) and another in VPC A.
3. CreateAutoScalingGroup in VPC A's subnets with DesiredCapacity=1 and TargetGroupARNs=[<VPC-B target group>].
4. Poll DescribeScalingActivities for an Activity with StatusCode='Cancelled' whose StatusMessage contains 'Failed to update target group': AWS yields one, the emulator never does.
5. DescribeTargetHealth on the mismatched group (AWS: empty), then detach it, attach the VPC-A group and confirm the next instance registers.
