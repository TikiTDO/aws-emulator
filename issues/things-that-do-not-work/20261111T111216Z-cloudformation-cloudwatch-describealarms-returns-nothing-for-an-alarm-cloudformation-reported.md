Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch.cfn-cloudwatch-alarm-in-place-update-retained-replacement-then-not-found
Issue-Key: cloudformation__cloudwatch.cfn-cloudwatch-alarm-in-place-update-retained-replacement-then-not-found|_assert_alarm#18|AssertionError|assert len(alarms) == 1, f"DescribeAlarms {name!r} returned {alarms!r}"
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# DescribeAlarms returns nothing for an alarm CloudFormation reported as created

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
Cause 1 of 1: an AWS::CloudWatch::Alarm provisioned by a stack is not visible through CloudWatch.

AWS: once a stack containing AWS::CloudWatch::Alarm reaches CREATE_COMPLETE, CloudWatch DescribeAlarms with AlarmNames=[<the alarm name>] returns exactly one MetricAlarm whose AlarmArn, MetricName, Namespace, Dimensions, Threshold, ComparisonOperator and Period match the template.

Emulator: DescribeAlarms for that name returns an empty MetricAlarms list — 'DescribeAlarms <alarm-name> returned []'. The alarm the stack claims to have created cannot be read, so neither the retained-replacement half of the scenario nor the post-delete not-found half is reachable.

Why: a client cannot read back, evaluate, SetAlarmState on, or attach actions to an alarm it created with CloudFormation; monitoring declared in a template simply does not exist in CloudWatch.

Fix: the AWS::CloudWatch::Alarm resource handler (it must call PutMetricAlarm against CloudWatch state) and/or the CloudWatch alarm store that DescribeAlarms reads.

Traces: the failing check in trace_ids. The open issue 20261109T223435Z-cloudformation-cloudwatch-alarms-a-stack-reports-create-complete.md records the same shape for other checks but does not carry this replay key.

## Evidence
Expected: DescribeAlarms(AlarmNames=['sim2real-cfnalarm-a-...']) returns exactly one MetricAlarm.
Actual: AssertionError: DescribeAlarms 'sim2real-cfnalarm-a-65ab0eeaf6ca' returned []

## Steps to Reproduce
1. CreateStack a template with an AWS::CloudWatch::Alarm (namespace/metric/dimension/threshold set); wait for CREATE_COMPLETE.
2. Call cloudwatch DescribeAlarms with that AlarmName — MetricAlarms is empty.
