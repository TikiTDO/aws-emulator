Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__ssm__xray.e2e-diagnose-xray-latency-cause-from-alarm-config-lifecycle cloudwatch__ecs__ssm__xray.fargate-task-role-without-xray-put-trace-segments-is-denied-the-frame-trace
Issue-Key: cloudwatch__ecs__ssm__xray.e2e-diagnose-xray-latency-cause-from-alarm-config-lifecycle|phase#12|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: cloudwatch__ecs__ssm__xray.fargate-task-role-without-xray-put-trace-segments-is-denied-the-frame-trace|_poll#31|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s; last {last!r}")
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# A CloudFormation-created AWS::CloudWatch::Alarm is invisible to DescribeAlarms, and a Fargate task's container never runs so no X-Ray segment or metric is ever emitted (2 causes)

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
Two net-new root causes behind both failing checks in this group.

Cause 1 of 2: an alarm provisioned by a CloudFormation stack is not registered with CloudWatch.
AWS: Smithy (cloudwatch) DescribeAlarms takes AlarmNames (len 0..100) and AlarmTypes ("If you omit this parameter, only metric alarms are returned") and returns MetricAlarms. A stack that reaches CREATE_COMPLETE with an AWS::CloudWatch::Alarm (Namespace, MetricName Time, ExtendedStatistic p90, Period, EvaluationPeriods 3, Threshold, ComparisonOperator GreaterThanThreshold, TreatMissingData notBreaching, two Dimensions) has that alarm in the account: DescribeAlarms(AlarmNames=[<name>], AlarmTypes=["MetricAlarm"]) returns exactly one MetricAlarm echoing the template.
Emulator: the stack reaches CREATE_COMPLETE and its Output Ref resolves to the alarm name, but that call returns an EMPTY MetricAlarms list. The DescribeAlarms handler filters correctly over its own store, so the alarm was never written into CloudWatch's alarm state by the resource provisioner (Hypothesis: the AWS::CloudWatch::Alarm resource handler records a stack resource without calling through to CloudWatch's PutMetricAlarm state).
Why: every client that deploys alarms as infrastructure and then reads or acts on them (DescribeAlarms, SetAlarmState, alarm actions, dashboards, incident tooling) sees no alarm, signalled only by an empty list rather than an error.
Fix: the CloudFormation AWS::CloudWatch::Alarm resource handler's propagation into CloudWatch's alarm state.
Similar open issues under other service headings: 20261109T185037Z-cloudformation-cloudwatch-sns-sqs-an-aws-cloudwatch-alarm-created-by-a-cdk.md and 20261029T182452Z-cloudwatch-sns-alarm-actions-cfn-alarms-are-never.md - neither carries these checks' replay keys.
Traces: e2e-diagnose-xray-latency-cause-from-alarm-config-lifecycle (phase 1/6 deploy).

Cause 2 of 2: a Fargate task accepted by RunTask never executes its container, so neither the metrics nor the X-Ray segments the container would emit ever exist.
AWS: Smithy (ecs) RunTask returns tasks (and failures); a FARGATE task with an awsvpc network configuration starts its container definitions. Here the control task's role allows xray:PutTraceSegments and the negative task's does not, so on AWS the allowed task's segments are retrievable through X-Ray (GetTraceSummaries) and both tasks publish their own counters through cloudwatch:PutMetricData, giving a non-zero positive control and a non-zero refusal count.
Emulator: RunTask answers one task and no failures for both task definitions, but no container ever runs - the polled triple (allowed writes, denied writes, traced segments) stays (0, 0, 0) for the full 420 s window, so neither the metric series nor any trace exists.
Why: any workload whose behaviour lives inside a container - emitting trace segments, publishing metrics, reading configuration, writing to a queue or bucket - produces nothing while the ECS control plane reports a healthy task, and every permission differential built on a running task is unobservable.
Fix: the ECS task runtime that is supposed to execute a task definition's container definitions (the RunTask/task-state execution path), not the RunTask request handler.
A similar open issue under another service heading: 20261104T060239Z-cloudwatch-ecr-ecs-fargate-tasks-never-leave-running.md - it does not carry this check's replay key.
Traces: fargate-task-role-without-xray-put-trace-segments-is-denied-the-frame-trace.

## Evidence
Cause 1 - cloudwatch__ecs__ssm__xray.e2e-diagnose-xray-latency-cause-from-alarm-config-lifecycle:
AssertionError: [phase 1/6 deploy] alarm 'sim2real-flintx-<token>-service-latency' should be described exactly once, got []
Read: cw.describe_alarms(AlarmNames=[<name>], AlarmTypes=["MetricAlarm"])["MetricAlarms"]
Expected (AWS): exactly one MetricAlarm echoing the template; Actual: [] although the stack is CREATE_COMPLETE and its alarm-name Output resolves.

Cause 2 - cloudwatch__ecs__ssm__xray.fargate-task-role-without-xray-put-trace-segments-is-denied-the-frame-trace:
AssertionError: the control tracing its frames and the denied task reporting refusals not met within 420s; last (0, 0, 0)
RunTask succeeded with no failures for both task definitions; the polled triple (allowed metric writes, denied metric writes, traced segments for the allowed task) never left (0, 0, 0).
Expected (AWS): non-zero writes on both series and at least one trace summary for the allowed task.

## Steps to Reproduce
Cause 1:
1. CreateStack with an AWS::CloudWatch::Alarm (ExtendedStatistic p90 over a custom namespace/metric with two dimensions) and an Output that Refs it. Wait for CREATE_COMPLETE.
2. Call cloudwatch:DescribeAlarms with AlarmNames=[<the Output value>] and AlarmTypes=["MetricAlarm"].
3. Observe MetricAlarms == [].

Cause 2:
1. RegisterTaskDefinition for a FARGATE task whose container emits X-Ray segments and CloudWatch metrics; give one task role xray:PutTraceSegments and withhold it from the other.
2. ecs:RunTask both definitions (awsvpc, assignPublicIp ENABLED); both succeed with no failures.
3. Poll cloudwatch:GetMetricStatistics and xray:GetTraceSummaries. Observe nothing is ever published or traced.
