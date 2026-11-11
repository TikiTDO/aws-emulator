Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__ecs.target-tracking-converges-to-the-target-and-stops application-autoscaling__ecs.two-policies-triggered-together-scale-by-the-largest-not-the-sum
Issue-Key: application-autoscaling__ecs.target-tracking-converges-to-the-target-and-stops|run#45|AssertionError|assert 1 <= len(scaled) <= 6, (
Issue-Key: application-autoscaling__ecs.two-policies-triggered-together-scale-by-the-largest-not-the-sum|_arbitrate#22|AssertionError|assert successful, (
Issue-Commit: 768db93f7ee9a823eb9c7336f7c11f317ed4b425

# application-autoscaling__ecs: a scaling activity that already moved ECS desiredCount never reaches StatusCode='Successful', so DescribeScalingActivities reports no successful activity at all

> _Note: this issue was identified against the code base at commit `768db93f7ee9a823eb9c7336f7c11f317ed4b425`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeScalingActivities

## Description
Cause 1 of 1: for an ecs:service:DesiredCount scalable target, Application Auto Scaling records the sizing activity but never advances its StatusCode to a terminal success, even long after ECS's own DescribeServices reports the new desiredCount. DescribeScalingActivities contains zero activities with StatusCode='Successful'.
AWS: ScalingActivity.StatusCode is required and drawn from ScalingActivityStatusCode = Pending|InProgress|Successful|Overridden|Unfulfilled|Failed (Smithy: application-autoscaling ScalingActivity.StatusCode -> ScalingActivityStatusCode, 'Indicates the status of the scaling activity'). A capacity change that the underlying service fulfils is a terminal success: the activity starts Pending/InProgress, gets an EndTime, and settles at StatusCode='Successful' with a StatusMessage describing the completed change. AAS's bookkeeping may lag ECS by a few seconds, but the terminal state is reached — a change that completed is never left non-terminal, and a change that cannot be fulfilled is reported as Unfulfilled or Failed, not held open.
Emulator: the activity exists with the right Description ('Setting desired count to <N>.') and ECS independently confirms desiredCount moved, but no activity in the list ever carries StatusCode='Successful' within the checks' bounded polls. Replay: `AssertionError: DescribeScalingActivities reports 0 successful activities reaching desiredCount 8 from 2: ['Setting desired count to 8.']. At least one is required (something must have moved capacity)...` and `AssertionError: leg 1: DescribeScalingActivities has no successful activity even though desiredCount moved to 14`. Both checks filter purely on StatusCode == 'Successful'.
Why: StatusCode is the only machine-readable completion signal in the scaling history. A client that waits for an activity to become Successful before proceeding — a deploy gate, a convergence check, a scale-out confirmation, or any dashboard that counts successful vs failed scaling events — waits forever on capacity that has in fact already moved, and cannot distinguish 'still working' from 'done' or from 'failed'. The requirement is the state machine, not a message: an activity whose capacity change was fulfilled must end with StatusCode='Successful' and an EndTime; the StatusMessage's variable parts (the resulting count, the service name) come from the request and state, its structure is fixed.
Fix: the scaling-activity lifecycle in the application-autoscaling crate — the state transition that must close an activity once the ECS capacity change is observed as applied (set EndTime, StatusCode='Successful', StatusMessage), and the serializer/state DescribeScalingActivities reads from.
Traces: application-autoscaling__ecs.target-tracking-converges-to-the-target-and-stops, application-autoscaling__ecs.two-policies-triggered-together-scale-by-the-largest-not-the-sum. (The open issue 20261108T003241Z-application-autoscaling-ecs-an-ecs-step-scaling-activity.md describes the INVERSE defect — a step-scaling activity recorded Successful immediately instead of staying InProgress — and does not carry these replay keys; a fix for this cause must leave the activity non-terminal until the change is applied and then make it terminal, satisfying both.)

## Evidence
Expected: after ECS DescribeServices reports the new desiredCount, application-autoscaling DescribeScalingActivities for the ecs:service:DesiredCount target contains at least one activity with StatusCode='Successful' (and the 'Setting desired count to <N>.' sizing record settles terminal with an EndTime).

Actual (check target-tracking-converges-to-the-target-and-stops): `AssertionError: DescribeScalingActivities reports 0 successful activities reaching desiredCount 8 from 2: ['Setting desired count to 8.']. At least one is required (something must have moved capacity) and more than six from 2 to 8 is thrashing, not proportional control` — the sizing activity is present by Description, but the list of StatusCode=='Successful' activities is empty.

Actual (check two-policies-triggered-together-scale-by-the-largest-not-the-sum): `AssertionError: leg 1: DescribeScalingActivities has no successful activity even though desiredCount moved to 14` — the check's filter is `[a for a in activities if a.get('StatusCode') == 'Successful']`, which yields nothing while ECS's desiredCount already reads 14.

## Steps to Reproduce
Create an ECS cluster and service; application-autoscaling RegisterScalableTarget(ServiceNamespace='ecs', ScalableDimension='ecs:service:DesiredCount', ResourceId='service/<cluster>/<service>'); attach a scaling policy (TargetTrackingScaling with a CustomizedMetricSpecification, or two StepScaling policies triggered together) and drive it until ECS DescribeServices reports the new desiredCount. Then call DescribeScalingActivities(ServiceNamespace='ecs', ResourceId='service/<cluster>/<service>') and inspect StatusCode across the returned activities: the 'Setting desired count to <N>.' record is present but no activity ever has StatusCode='Successful', however long you poll.
