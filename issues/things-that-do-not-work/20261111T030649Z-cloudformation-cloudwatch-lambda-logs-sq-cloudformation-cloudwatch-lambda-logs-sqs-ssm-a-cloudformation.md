Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch__lambda__logs__sqs__ssm.cfn-change-set-queue-worker-update-workflow
Issue-Key: cloudformation__cloudwatch__lambda__logs__sqs__ssm.cfn-change-set-queue-worker-update-workflow|_alarm#22|AssertionError|assert alarms, f"alarm {name} does not exist"
Issue-Commit: 6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf

# cloudformation__cloudwatch__lambda__logs__sqs__ssm: a CloudFormation-deployed AWS::CloudWatch::Alarm is invisible to DescribeAlarms, so a change-set update workflow can never observe its alarm

> _Note: this issue was identified against the code base at commit `6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
One net-new root cause. This check is not marked ALREADY FILED and is not OWNED by an open issue; open issues of a similar shape exist for other service families (e.g. 20261110T232635Z-cloudwatch-ecs-ssm-a-cloudformation-created-aws-cloudwatch-alarm-is-invisible.md) but none carries this check's replay key.

Cause 1 of 1: an AWS::CloudWatch::Alarm created by a CREATE_COMPLETE/UPDATE_COMPLETE CloudFormation stack is not registered in CloudWatch.
AWS: once a stack containing an AWS::CloudWatch::Alarm reaches CREATE_COMPLETE (or UPDATE_COMPLETE after a change-set execution), the alarm exists in CloudWatch. DescribeAlarms with AlarmNames=[<the alarm's physical id, which for AWS::CloudWatch::Alarm is its AlarmName>] returns exactly one MetricAlarm whose StateValue is one of INSUFFICIENT_DATA, OK or ALARM, and that alarm then transitions to ALARM once its metric breaches the declared threshold for the declared number of evaluation periods. Smithy: cloudwatch DescribeAlarms -> DescribeAlarmsOutput.MetricAlarms (list of MetricAlarm with required-for-use members AlarmName, StateValue of enum StateValue = OK|ALARM|INSUFFICIENT_DATA); recorded behavior for the CFN side.
Emulator: after the stack and the change-set execution report success, DescribeAlarms returns an empty MetricAlarms list for the stack's alarm, so the check dies on `AssertionError: alarm <alarm-name> does not exist` and the alarm can never be polled to ALARM. The alarm name in that message is a per-run generated value; the requirement is that the alarm the stack declared is retrievable at all.
Why: a client that deploys its alarms through CloudFormation (or through a change set) and then reads or waits on them through CloudWatch sees them as nonexistent. Every downstream wait on alarm state, every alarm action, and every console/CLI listing of the stack's alarms is wrong, while the stack itself reports success — the worst combination, because nothing signals the loss.
Fix: the CloudFormation AWS::CloudWatch::Alarm resource handler — it must create the alarm in the CloudWatch service state (the same state DescribeAlarms/PutMetricAlarm read and write), on both create and change-set update, instead of only recording a stack resource.
Traces: cloudformation__cloudwatch__lambda__logs__sqs__ssm.cfn-change-set-queue-worker-update-workflow.

## Evidence
Cause 1 — Expected: after the stack/change-set reports success, DescribeAlarms(AlarmNames=[<alarm name declared in the template>]) returns one MetricAlarm with a StateValue in {INSUFFICIENT_DATA, OK, ALARM}, which then reaches ALARM once the metric breaches. Actual: DescribeAlarms returns no alarm at all; the check fails with `AssertionError: alarm sim2real-xsvc-alarm-3fa3bf351cea does not exist` raised from its `_alarm()` helper (`assert alarms, f"alarm {name} does not exist"`), so `_assert_alarm_reaches_alarm` never gets a StateValue.

## Steps to Reproduce
1. CreateStack with a template declaring an SQS queue, a Lambda worker, and an AWS::CloudWatch::Alarm over a queue/worker metric; wait for CREATE_COMPLETE.
2. CreateChangeSet + ExecuteChangeSet for the queue-worker update; wait for UPDATE_COMPLETE.
3. Call cloudwatch DescribeAlarms with AlarmNames=[<the AlarmName in the template>].
Observed: MetricAlarms is empty. Expected: one MetricAlarm for that name, whose StateValue later becomes ALARM once the metric breaches the declared threshold.
