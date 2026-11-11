Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__ecs.a-metric-that-never-breaches-produces-no-scaling-activity
Issue-Key: application-autoscaling__ecs.a-metric-that-never-breaches-produces-no-scaling-activity|run#29|Timeout|time.sleep(_PUBLISH_INTERVAL_S)
Issue-Commit: 14a884b09fa9584b0ed8d02922f2a6aac4ab163d

# application-autoscaling__ecs: an alarm-driven StepScaling policy never scales an ECS service, so the never-breaching/control scenario never reaches a terminal state

> _Note: this issue was identified against the code base at commit `14a884b09fa9584b0ed8d02922f2a6aac4ab163d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
Cause 1 of 1: a StepScaling policy on `ecs:service:DesiredCount` that is named as the AlarmAction of a CloudWatch metric alarm never produces the scale-in it is wired for, so the whole alarm -> policy -> ECS UpdateService -> scaling-activity path never reaches a terminal state.

AWS: Smithy (application-autoscaling) — PutScalingPolicy with StepScalingPolicyConfiguration/StepAdjustment{MetricIntervalUpperBound, ScalingAdjustment} returns a PolicyARN plus the Alarms it participates in; StepAdjustment doc: "A positive value adds to the current capacity while a negative number removes from the current capacity", the adjustment being selected from the difference between the aggregated CloudWatch metric and the alarm's breach threshold. ScalingActivity (required ActivityId/ServiceNamespace/ResourceId/ScalableDimension/Description/Cause/StartTime/StatusCode, optional StatusMessage, optional NotScaledReasons; ScalingActivityStatusCode = Pending|InProgress|Successful|Overridden|Unfulfilled|Failed) is what DescribeScalingActivities(IncludeNotScaledActivities=true) returns. The per-namespace activity wording is recorded behavior only: when a LessThanThreshold alarm whose AlarmActions name the policy goes to ALARM on a published metric, Application Auto Scaling executes the matching negative step adjustment, calls ECS so that DescribeServices reports a desiredCount strictly between MinCapacity and the pre-scale count, and records ONE activity for that target whose StatusCode stays `InProgress` on an estate with no container instances (ECS can never finish placing/removing tasks), whose Description is the fixed text `Setting desired count to ` + the computed count + `.`, whose StatusMessage is the fixed text `Successfully set desired count to `, `. Waiting for change to be fulfilled by `, `.` with the computed count and the fulfilling namespace (`ecs`) as the variable parts, and whose Cause names both the CloudWatch alarm and the policy that fired (both values coming from the request/state). Symmetrically, a target whose alarm never breaches (the published value sits on the non-breaching side of its own threshold) keeps its desiredCount and has NO activity at all — not even a not-scaled one — while DescribeAlarms reports that alarm `OK`.

Emulator: none of that is ever observed. Every API call in the scenario was answered (no ClientError and no AssertionError was raised), but the check was killed by the harness after 60s while inside one of its bounded polls, i.e. the condition it waits for never became true. Hypothesis: the CloudWatch alarm fed by PutMetricData is never evaluated into ALARM and/or its AlarmActions are never dispatched to Application Auto Scaling, so the positive-control leg (publishing 50.0 against a LessThanThreshold threshold of 90.0 and waiting up to 120s for the control ECS service to move off desiredCount 6) spins until the budget expires; the earlier polls (ECS reporting desiredCount 6 for each freshly created service) and the later repair leg are the other candidates and the suite report does not isolate which poll hung. The suite does not report the state the emulator did reach, only that the scenario never terminated.

Why: a client that wires the documented ECS step-scaling topology (RegisterScalableTarget -> PutScalingPolicy -> PutMetricAlarm with the PolicyARN as an AlarmAction -> PutMetricData) sees every call succeed and then sees capacity never move and DescribeScalingActivities stay empty. Autoscaling behaviour becomes untestable against the emulator in BOTH directions: a breach that must scale does nothing, and the negative case (a metric that never breaches must produce no activity) is indistinguishable from a target that has no scaling engine at all, so a correct absence can never be demonstrated.

Fix: the step-scaling execution path — the CloudWatch alarm-evaluation/alarm-action dispatch in the cloudwatch service (PutMetricData -> alarm state transition -> AlarmActions) and the scaling engine behind PutScalingPolicy in the application-autoscaling service (services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/put_scaling_policy.rs and the activity/state component it writes through), including the ECS-namespace downstream write of desiredCount and the `InProgress` activity with the ecs-fulfilment-pending Description/StatusMessage/Cause above. Related but distinct: open issue 20261031T230626Z-application-autoscaling-lambda-registering-a-lambda-function-provisionedconcurrency.md and cause 7 of 20261029T232940Z-application-auto-scaling-7-net-new-root-causes.md cover the REGISTRATION-floor path (no policy involved) for the lambda/custom-resource/cassandra namespaces; this one is the alarm-driven StepScaling policy path for the `ecs` namespace. Fixed issue 20261029T004004Z-application-autoscaling-ecs-application-auto-scaling-scaling-activities-carry-no-statusmessage.md covers the StatusMessage text of an ECS step-scaling activity; here no activity and no capacity change is reached at all.

Traces: application-autoscaling__ecs.a-metric-that-never-breaches-produces-no-scaling-activity.

## Evidence
Suite verdict: [mismatch] application-autoscaling__ecs.a-metric-that-never-breaches-produces-no-scaling-activity — "Check timed out after 60.0s". No ClientError and no AssertionError were raised, so every call the scenario made was answered; one of its bounded polls never saw its condition.

Scenario (two identical ECS estates on clusters with no container instances, start desiredCount 6, Min 1 / Max 40, one StepScaling policy per target with StepAdjustment{MetricIntervalUpperBound: 0.0, ScalingAdjustment: -3}, Cooldown 0, and one LessThanThreshold alarm per target on namespace 'sim2real/behav/application-autoscaling', metric 'NonBreachDriver', Statistic Maximum, Period 10, EvaluationPeriods 1, TreatMissingData notBreaching, AlarmActions=[PolicyARN]; the single value 50.0 is published to both streams):

Expected (control, threshold 90.0 — breaches):
  - ECS DescribeServices(cluster, [service]).services[0].desiredCount leaves 6 and settles at some value c with 1 <= c < 6
  - CloudWatch DescribeAlarms(AlarmNames=[control alarm]).MetricAlarms[0].StateValue == 'ALARM'
Expected (subject, threshold 10.0 — never breaches):
  - DescribeAlarms StateValue stays 'OK' for the whole hold window
  - ECS desiredCount stays 6
  - DescribeScalingActivities(ServiceNamespace='ecs', ResourceId='service/<cluster>/<service>', ScalableDimension='ecs:service:DesiredCount', IncludeNotScaledActivities=True).ScalingActivities == [] (no record at all, not even a not-scaled one)
Expected (repair: subject threshold raised to 90.0 on the same alarm, same value still flowing):
  - the subject's desiredCount leaves 6 for some value s with 1 <= s < 6
  - the newest activity has StatusCode 'InProgress', Description 'Setting desired count to <s>.', StatusMessage 'Successfully set desired count to <s>. Waiting for change to be fulfilled by ecs.', and a Cause naming both the alarm name and the policy name that fired

Actual: none of these conditions was ever observed. The scenario hung and was terminated at the 60s harness budget, with no error from any call. The emulator therefore demonstrated neither the breach-driven scale-in nor the terminal activity record.

Note on attribution: the check's own windows are generous (120s for the control leg, >=90s for the absence hold), so the 60s budget means the report cannot name the exact poll that hung; what is grounded is that the scenario reached no terminal state and no call was refused.

## Steps to Reproduce
1. Create two ECS clusters with NO container instances; register a bridge/EC2 task definition in each and CreateService(desiredCount=6, launchType='EC2') in each. Wait for DescribeServices to report desiredCount 6 for both.
2. For each service: RegisterScalableTarget(ServiceNamespace='ecs', ResourceId='service/<cluster>/<service>', ScalableDimension='ecs:service:DesiredCount', MinCapacity=1, MaxCapacity=40) and assert DescribeScalingActivities(IncludeNotScaledActivities=True) is empty.
3. For each: PutScalingPolicy(PolicyType='StepScaling', StepScalingPolicyConfiguration={'AdjustmentType':'ChangeInCapacity','MetricAggregationType':'Maximum','Cooldown':0,'StepAdjustments':[{'MetricIntervalUpperBound':0.0,'ScalingAdjustment':-3}]}) and keep the PolicyARN.
4. For each: PutMetricAlarm(Namespace='sim2real/behav/application-autoscaling', MetricName='NonBreachDriver', Dimensions=[{'Name':'ServiceName','Value':<service>}], Statistic='Maximum', Period=10, EvaluationPeriods=1, DatapointsToAlarm=1, ComparisonOperator='LessThanThreshold', TreatMissingData='notBreaching', AlarmActions=[<PolicyARN>]) with Threshold 90.0 for the control and 10.0 for the subject. Verify DescribeAlarms echoes the AlarmActions and Threshold.
5. Publish PutMetricData Value=50.0 to both streams every 5s. Within 120s the CONTROL service's desiredCount must drop below 6 (bounded below by MinCapacity 1) and its alarm must read ALARM. On the emulator this never happens and the scenario hangs.
6. Hold the same publishing for >=90s and assert the SUBJECT alarm stays OK, its desiredCount stays 6, and DescribeScalingActivities(IncludeNotScaledActivities=True) for it is still [].
7. Repair: PutMetricAlarm the subject alarm with Threshold 90.0, keep publishing 50.0, and within 120s expect the subject's desiredCount to drop below 6 and its newest activity to be StatusCode 'InProgress' with Description 'Setting desired count to <count>.', StatusMessage 'Successfully set desired count to <count>. Waiting for change to be fulfilled by ecs.' and a Cause naming the alarm and the policy.
