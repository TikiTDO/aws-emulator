Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__cloudwatch__ec2__ecs__elbv2.step-scaling-continuing-alarm-adds-nothing-until-the-cooldown-ends
Issue-Key: application-autoscaling__cloudwatch__ec2__ecs__elbv2.step-scaling-continuing-alarm-adds-nothing-until-the-cooldown-ends|_poll_until#25|AssertionError|raise AssertionError(f'{description} not observed within {timeout_s}s; last saw {last!r}')
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# application-autoscaling: a step-scaling policy whose alarm stays in ALARM never applies its next step after the cooldown expires

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: an Application Auto Scaling step-scaling policy that is re-invoked by a CloudWatch alarm which REMAINS in ALARM adds nothing once its Cooldown has expired — the scalable target's capacity freezes at the first step.
AWS: Smithy (application-autoscaling) PutScalingPolicy -> PolicyType 'StepScaling' with StepScalingPolicyConfiguration.Cooldown, documented as the amount of time, in seconds, to wait for a previous scaling activity to take effect; StepAdjustment.ScalingAdjustment is the increment applied per breach. RegisterScalableTarget/DescribeScalableTargets expose the ecs:service:DesiredCount dimension the check reads. AWS behaviour: while the alarm stays in ALARM it re-invokes the policy roughly once a minute; invocations inside the cooldown window are suppressed, and the FIRST invocation after the cooldown has elapsed applies the next step adjustment, so DesiredCount advances again (bounded by MaxCapacity). The suppression-then-resume rule is the requirement; the particular numbers (cooldown seconds, step size, min/max capacity) come from the request.
Emulator: after the first step lands (DesiredCount moved to its post-first-step value), the still-ALARM alarm produces no further scaling activity at all. The check polls DescribeScalableTargets for a desired count greater than the first one and times out: AssertionError "the continuing alarm adding the next step after the 120s cooldown (alarm ALARM) not observed within 270.0s; last saw 2" — the 120 s, 270 s and 2 are this run's cooldown, ceiling and observed capacity; the defect is that the value never changes again.
Why: a client that relies on step scaling to keep adding capacity while a breach persists sees a single step and then a flat line. Capacity never reaches the level the policy's steps and MaxCapacity describe, so autoscaling under sustained load is silently a no-op after the first action, and any test of cooldown semantics passes for the wrong reason (nothing ever happens, so nothing ever happens too early either).
Fix: the Application Auto Scaling step-scaling component — the alarm-driven policy invocation path and the cooldown gate in the scaling-activity scheduler: a policy must stay subscribed to its alarm's ALARM state and re-evaluate (and apply) its step adjustment on the first invocation after Cooldown elapses, not only once per alarm transition.
Traces: application-autoscaling__cloudwatch__ec2__ecs__elbv2.step-scaling-continuing-alarm-adds-nothing-until-the-cooldown-ends

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator.

[mismatch] application-autoscaling__cloudwatch__ec2__ecs__elbv2.step-scaling-continuing-alarm-adds-nothing-until-the-cooldown-ends
  File "check_...step_scaling_continuing_alarm_adds_nothing_until_the_cooldown_ends.py", line 368, in run
    final = _poll_until(probe, lambda d: d > first_desired, timeout_s=_SECOND_ACTION_S, ...)
  AssertionError: the continuing alarm adding the next step after the 120s cooldown (alarm ALARM) not observed within 270.0s; last saw 2

Expected (AWS): with the alarm still in ALARM and the policy's Cooldown (120 s here) elapsed, the next step adjustment is applied and DesiredCount rises above the first-step value, up to MaxCapacity.
Actual (emulator): DesiredCount stays at the first-step value for the whole 270 s window (cooldown + the alarm's re-invocation interval + activity time); no second scaling activity is ever recorded.

Smithy consulted: application-autoscaling PutScalingPolicy input {PolicyName, ServiceNamespace, ResourceId, ScalableDimension 'ecs:service:DesiredCount', PolicyType, StepScalingPolicyConfiguration {AdjustmentType, StepAdjustments[].ScalingAdjustment, Cooldown, MetricAggregationType}}; DescribeScalableTargets output ScalableTargets[].{MinCapacity,MaxCapacity}.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; the check id in trace_ids reproduces.
2. Create an ECS service and RegisterScalableTarget on ecs:service:DesiredCount with MinCapacity=1, MaxCapacity=3.
3. PutScalingPolicy with PolicyType='StepScaling' and StepScalingPolicyConfiguration={AdjustmentType:'ChangeInCapacity', Cooldown:120, StepAdjustments:[{MetricIntervalLowerBound:0, ScalingAdjustment:1}]}.
4. Create a CloudWatch alarm on a custom metric with the policy ARN as its AlarmAction and keep publishing a breaching datapoint so the alarm stays in ALARM.
5. Observe DesiredCount via DescribeScalableTargets. The first step lands. Then wait past the 120 s cooldown and keep the alarm in ALARM.
6. Expected: DesiredCount advances by another step (to MaxCapacity over successive invocations). Actual: it never changes again within Cooldown + 150 s.
7. Re-replay the id in trace_ids to confirm the fix.
