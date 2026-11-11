Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action|_receive#27|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Commit: 3163fad6f190575ee3da7920eeb1a958f0b14066

# cloudwatch: a SetAlarmState transition delivers no notification to the alarm's SNS AlarmAction, so the ActionsEnabled suppress/restore contract is never observable

> _Note: this issue was identified against the code base at commit `3163fad6f190575ee3da7920eeb1a958f0b14066`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetAlarmState

## Description
Cause 1 of 1: forcing an alarm into ALARM with SetAlarmState does not deliver the alarm's AlarmActions notification, so the check that proves DisableAlarmActions suppresses delivery and EnableAlarmActions restores it never reaches a verdict.

AWS: Smithy: SetAlarmState(AlarmName, StateValue, StateReason[, StateReasonData]) — "Temporarily sets the state of an alarm for testing purposes. … the alarm's state … and its actions are executed"; DisableAlarmActions/EnableAlarmActions flip MetricAlarm.ActionsEnabled, which DescribeAlarms reports. For an alarm whose AlarmActions is an SNS topic ARN, a transition into ALARM while ActionsEnabled=true publishes one notification to that topic, whose Message is the alarm JSON carrying AlarmName, NewStateValue="ALARM" and NewStateReason equal to the StateReason given on the SetAlarmState call; with ActionsEnabled=false the transition still changes the state but publishes nothing. The recording this check is calibrated against received the positive-control notification well inside 90 s and then sized its absence window from that measured latency.

Emulator: the check exhausts its budget and is reported as `Error: Check timed out after 60.0s`. Its first awaited effect is the positive control — one SNS→SQS message after the very first SetAlarmState(ALARM), before ActionsEnabled is touched at all — so the timeout is reached while waiting for a notification that never arrives. Hypothesis: alarm state transitions driven by SetAlarmState do not execute AlarmActions (the same shape the open cloudwatch alarm-action issues report for CFN-created and metric-driven alarms).

Why: a client that uses SetAlarmState to rehearse an alarm, or that relies on ActionsEnabled to mute and unmute paging, observes nothing: no notification on ALARM, and the suppress/restore semantics cannot be distinguished from a permanently broken action. Runbooks and incident wiring validated against the emulator will silently never page.

Fix: the alarm state-transition action dispatch in the CloudWatch alarm engine (the path SetAlarmState shares with evaluation-driven transitions), gated on the alarm's ActionsEnabled flag; not the check.

Related open issues that do NOT cover this check (the suite did not mark it owned, so it is filed separately): 20261029T182452Z-cloudwatch-sns-alarm-actions-cfn-alarms-are-never.md and 20261109T171600Z-cloudwatch-alarm-state-transitions-emit-no-cloudwatch-alarm.md.

Traces: cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action.

## Evidence
Expected: PutMetricAlarm(AlarmActions=[topic]) → SetAlarmState(StateValue=ALARM, StateReason="behavioral-check-baseline") delivers exactly one SNS message to the subscribed SQS queue whose Message.AlarmName is the alarm and whose Message.NewStateReason is that reason; after DisableAlarmActions a fresh ALARM transition delivers nothing and ApproximateNumberOfMessages stays 0; after EnableAlarmActions the next ALARM transition delivers exactly one message with NewStateValue="ALARM" and NewStateReason="behavioral-check-restored".

Actual: `[mismatch] cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action` → `Error: Check timed out after 60.0s`; the positive-control receive (up to 90 s) never yields the baseline notification, so none of the suppression or restoration assertions are ever evaluated.

## Steps to Reproduce
1. sns CreateTopic; sqs CreateQueue; SetQueueAttributes with a policy allowing sns.amazonaws.com sqs:SendMessage from that topic; sns Subscribe(Protocol=sqs).
2. cloudwatch PutMetricAlarm(AlarmName=<name>, Namespace=<ns>, MetricName=Errors, Statistic=Average, Period=60, EvaluationPeriods=1, Threshold=1.0, ComparisonOperator=GreaterThanThreshold, TreatMissingData=missing, AlarmActions=[<topic arn>]).
3. cloudwatch SetAlarmState(AlarmName=<name>, StateValue=ALARM, StateReason="behavioral-check-baseline") and ReceiveMessage on the queue.
Observed: no message arrives. Expected: one message whose Message.NewStateReason is "behavioral-check-baseline".
4. (Unreached) DisableAlarmActions → ALARM transition delivers nothing; EnableAlarmActions → next ALARM transition delivers exactly one notification.
