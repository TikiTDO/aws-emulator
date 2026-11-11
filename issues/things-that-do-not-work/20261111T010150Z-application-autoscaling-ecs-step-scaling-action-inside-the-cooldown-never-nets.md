Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__ecs.step-scaling-cooldown-nets-the-prior-step-then-adds-the-whole-step-after-it
Issue-Key: application-autoscaling__ecs.step-scaling-cooldown-nets-the-prior-step-then-adds-the-whole-step-after-it|_poll_until#30|AssertionError|raise AssertionError(f'{description} not observed within {timeout_s}s; last saw {last!r}')
Issue-Commit: dff6e291e5e6489707888b0049fa49b367909ca3

# Step-scaling action inside the cooldown never nets the prior step: desiredCount frozen until the cooldown ends

> _Note: this issue was identified against the code base at commit `dff6e291e5e6489707888b0049fa49b367909ca3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
Cause 1 of 1: a step-scaling policy whose alarm re-breaches into a HIGHER step bucket while the previous action's cooldown is still running produces no further capacity change at all.

AWS: recorded behavior only for the exact arithmetic (the Smithy model for Application Auto Scaling describes PutScalingPolicy/StepScalingPolicyConfiguration + Cooldown but states no cooldown algebra). AWS documents, and the recording shows, that during a scale-out cooldown a new breach is NOT simply suppressed: the next action is evaluated and the capacity already added by the in-flight step is NETTED OUT, so a breach that lands above a larger step boundary moves desiredCount again inside the cooldown window by the remainder. The recording shows desiredCount leaving the value the first step produced while the 180 s cooldown is still in force, then the whole step being added once the cooldown expires.

Emulator: desiredCount does not move at all inside the cooldown. The replayed check watched it stay pinned at 6 for the full 150 s window after a metric breach that crossed the 20.0 step boundary ("leg 2: desiredCount moving off 6 on a breach above the 20.0 step boundary inside the 180s cooldown not observed within 150.0s; last saw 6"). The emulator is treating the cooldown as a blanket suppression window instead of netting the in-flight step.

Why: a client that sizes its ECS service with step scaling under a worsening load gets no capacity for the remainder of the cooldown. Autoscaling test suites and runbooks that assert the netted intermediate capacity (the behavior AWS actually implements) conclude the policy is broken, and load-shedding simulations under-provision by exactly one step.

Fix: the scaling-decision component of the application-autoscaling service — the step-scaling evaluator that applies Cooldown. It must, for a scale-out action raised during an active cooldown, compute the new step's adjustment and subtract the capacity the in-flight action already contributed, applying any positive remainder immediately, rather than discarding the action.

Traces: application-autoscaling__ecs.step-scaling-cooldown-nets-the-prior-step-then-adds-the-whole-step-after-it. Related but distinct open issues already filed for this service: 20261108T003241Z-application-autoscaling-ecs-an-ecs-step-scaling-activity.md and 20261109T144605Z-application-autoscaling-ecs-2-net-new-causes-an.md — neither carries this check's replay key.

## Evidence
Expected (AWS, from the recording): after the first step takes desiredCount to 6, a metric breach above the 20.0 step boundary raised INSIDE the 180 s cooldown moves desiredCount off 6 (the prior step netted out), and after the cooldown the whole step is added.
Actual (emulator): desiredCount remains 6 for the entire 150 s observation window; no scaling activity is recorded for the in-cooldown breach. AssertionError: 'leg 2: desiredCount moving off 6 on a breach above the 20.0 step boundary inside the 180s cooldown not observed within 150.0s; last saw 6'.

## Steps to Reproduce
1. Register an ECS service as a scalable target (ServiceNamespace=ecs, ScalableDimension=ecs:service:DesiredCount).
2. PutScalingPolicy with PolicyType=StepScaling, several step adjustments with increasing MetricIntervalLowerBound, and Cooldown=180.
3. Breach the lower step so the policy fires and desiredCount rises (e.g. to 6).
4. While the 180 s cooldown is still running, publish a metric value that falls into a higher step bucket (above the 20.0 boundary) and keep the alarm in ALARM.
5. Poll DescribeScalableTargets/DescribeServices for desiredCount for ~150 s: AWS moves it off 6 inside the cooldown; the emulator leaves it at 6.
