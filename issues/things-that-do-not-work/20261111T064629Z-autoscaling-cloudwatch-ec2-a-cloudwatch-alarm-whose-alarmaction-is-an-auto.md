Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudwatch__ec2.alarm-state-change-drives-the-scaling-policy-to-launch-an-instance autoscaling__cloudwatch__ec2.target-tracking-high-resolution-custom-metric-scales-out-by-the-ratio
Issue-Key: autoscaling__cloudwatch__ec2.alarm-state-change-drives-the-scaling-policy-to-launch-an-instance|_poll#20|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s; last={last!r}")
Issue-Key: autoscaling__cloudwatch__ec2.target-tracking-high-resolution-custom-metric-scales-out-by-the-ratio|_poll#23|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s; last={last!r}")
Issue-Commit: 4315fe29349a8ccf086f6caac897ebb4dc04700d

# A CloudWatch alarm whose AlarmAction is an Auto Scaling policy ARN is never handed off: no Action history item and DesiredCapacity never moves (2 causes)

> _Note: this issue was identified against the code base at commit `4315fe29349a8ccf086f6caac897ebb4dc04700d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
Two net-new causes on the same boundary: the alarm reaches ALARM, but the CloudWatch->Auto Scaling action hand-off never happens — neither CloudWatch's own record of it (Cause 1) nor Auto Scaling's response to it (Cause 2).

Cause 1 of 2 — CloudWatch records no Action history item for an alarm action that is a scaling-policy ARN.
AWS: Smithy: PutMetricAlarm.AlarmActions is a ResourceList of action ARNs and an Auto Scaling PolicyARN (as returned by autoscaling PutScalingPolicy) is a valid entry; DescribeAlarmHistory exposes HistoryItemType=Action items. AWS documents (https://docs.aws.amazon.com/AmazonCloudWatch/latest/monitoring/AlarmThatSendsEmail.html) that when an alarm transitions to ALARM it invokes each configured action and writes an Action history item naming the invoked action ARN, whose HistoryData carries actionState=Succeeded once the invocation was accepted. Recorded behavior: after the genuine breach drives the alarm to ALARM, DescribeAlarmHistory for that alarm contains an Action item naming the policy ARN with actionState "Succeeded" within minutes.
Emulator: the alarm does reach ALARM from the published datapoint, but DescribeAlarmHistory never yields any Action item for the policy ARN — the filtered history list stays empty for the full 300 s budget (last=[]).
Why: a client cannot tell whether its alarm ever fired. Operators and automation audit the Action history to confirm an alarm acted; here every alarm looks like it evaluated but never did anything, so a failed scaling hand-off is indistinguishable from a healthy one.
Fix: the CloudWatch alarm evaluation/action component — on a state transition it must dispatch each AlarmAction/OKAction/InsufficientDataAction and append the corresponding Action history item (with its actionState) to the alarm's history, for scaling-policy ARNs as for any other action target.

Cause 2 of 2 — Auto Scaling never executes the policy the alarm names, so DesiredCapacity never changes.
AWS: Smithy: autoscaling PutScalingPolicy admits PolicyType SimpleScaling (AdjustmentType ChangeInCapacity, ScalingAdjustment) and TargetTrackingScaling (TargetTrackingConfiguration with a CustomizedMetricSpecification and TargetValue); for target tracking the service itself creates the AlarmHigh/AlarmLow alarms acting on the policy. AWS documents that when such an alarm is in ALARM the policy executes: a SimpleScaling ChangeInCapacity policy raises DesiredCapacity by its ScalingAdjustment, and a target-tracking policy moves DesiredCapacity toward ceil(current * metric / TargetValue), recording a scaling activity that names the policy. Recorded behavior: with the subject fed a metric of 3x its TargetValue, DesiredCapacity moves away from 1 (to the ratio, 3) while the at-target control group stays at 1.
Emulator: the service-created AlarmHigh reaches ALARM ('subject_high': 'ALARM', control 'OK'), yet DesiredCapacity stays at the original value on both groups ('subject_desired': 1, 'control_desired': 1) for the full 240 s budget, and no scaling activity naming the policy is recorded.
Why: every alarm-driven scaling path is dead. A client that provisions capacity through step/simple/target-tracking policies gets a group that never scales out under load and never scales in, while the alarms it inspects say the breach was detected — so autoscaling behaviour under load cannot be exercised or tested at all.
Fix: the Auto Scaling scaling-policy execution component — the path that consumes an alarm action for a PolicyARN (ExecutePolicy semantics) must apply the policy's adjustment to the group's DesiredCapacity, honour Cooldown, and record a scaling activity naming the policy; it must be reachable from the CloudWatch action dispatch repaired in Cause 1.

Traces: autoscaling__cloudwatch__ec2.alarm-state-change-drives-the-scaling-policy-to-launch-an-instance (Cause 1), autoscaling__cloudwatch__ec2.target-tracking-high-resolution-custom-metric-scales-out-by-the-ratio (Cause 2).

## Evidence
Cause 1 — check autoscaling__cloudwatch__ec2.alarm-state-change-drives-the-scaling-policy-to-launch-an-instance [mismatch]:
AssertionError: CloudWatch recorded invoking sim2real-xsvc-pol-<id> for alarm sim2real-xsvc-alarm-<id> not met within 300s; last=[]
Expected: a DescribeAlarmHistory item of HistoryItemType=Action naming the policy ARN, with actionState=Succeeded, after the alarm transitioned to ALARM (names/ids are per-run values).
Actual: the Action-history list stayed empty for 300 s even though the alarm was in ALARM.

Cause 2 — check autoscaling__cloudwatch__ec2.target-tracking-high-resolution-custom-metric-scales-out-by-the-ratio [mismatch]:
AssertionError: the subject fed 150.0 against target 50.0 changed DesiredCapacity away from 1 not met within 240s; last={'subject_desired': 1, 'control_desired': 1, 'subject_high': 'ALARM', 'control_high': 'OK'}
Expected: DesiredCapacity rises to ceil(1 * 150 / 50) = 3 on the breaching group while the at-target control stays at 1.
Actual: AlarmHigh is in ALARM but DesiredCapacity stays 1 on both groups.

## Steps to Reproduce
Cause 1:
1. Create an ASG with one InService instance.
2. autoscaling PutScalingPolicy(PolicyType=SimpleScaling, AdjustmentType=ChangeInCapacity, ScalingAdjustment=1, Cooldown=0); keep the returned PolicyARN.
3. cloudwatch PutMetricAlarm on a custom metric with AlarmActions=[PolicyARN], Period=60, EvaluationPeriods=1, DatapointsToAlarm=1, TreatMissingData=notBreaching.
4. PutMetricData a breaching datapoint until DescribeAlarms reports ALARM.
5. DescribeAlarmHistory(AlarmName, HistoryItemType=Action): AWS lists an item naming the PolicyARN with actionState Succeeded; the emulator lists none.

Cause 2:
1. Create two identical ASGs (desired 1, max 4), each with a TargetTrackingScaling policy over a custom metric at a 10 s period and TargetValue=50.
2. PutMetricData 150 for the subject and 50 for the control every 5 s.
3. Poll DescribeAutoScalingGroups: AWS moves the subject's DesiredCapacity to 3 and leaves the control at 1; the emulator leaves both at 1 while the subject's AlarmHigh reads ALARM.
