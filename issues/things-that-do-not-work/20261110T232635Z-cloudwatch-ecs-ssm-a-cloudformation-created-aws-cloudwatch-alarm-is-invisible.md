Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__ssm.e2e-diagnose-cloudwatch-latency-breach-from-alarm-config-lifecycle cloudwatch__ecs__ssm.fargate-task-role-without-ssm-get-parameter-is-denied-the-latency-flag
Issue-Key: cloudwatch__ecs__ssm.e2e-diagnose-cloudwatch-latency-breach-from-alarm-config-lifecycle|phase#11|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: cloudwatch__ecs__ssm.fargate-task-role-without-ssm-get-parameter-is-denied-the-latency-flag|_poll#30|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s; last {last!r}")
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# A CloudFormation-created AWS::CloudWatch::Alarm is invisible to DescribeAlarms, and a Fargate task's container never runs (2 causes)

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
Two net-new root causes behind both failing checks in this group.

Cause 1 of 2: an alarm provisioned by a CloudFormation stack is not registered with CloudWatch.
AWS: Smithy (cloudwatch) DescribeAlarms takes AlarmNames (len 0..100) and AlarmTypes ("If you omit this parameter, only metric alarms are returned") and returns MetricAlarms. A stack that reaches CREATE_COMPLETE with an AWS::CloudWatch::Alarm (here Namespace, MetricName Time, ExtendedStatistic p90, Period, EvaluationPeriods 3, Threshold, ComparisonOperator GreaterThanThreshold, TreatMissingData notBreaching, two Dimensions) has that alarm in the account: DescribeAlarms(AlarmNames=[<name>], AlarmTypes=["MetricAlarm"]) returns exactly one MetricAlarm whose configuration echoes the template.
Emulator: the stack reaches CREATE_COMPLETE and its Output Ref resolves to the alarm name, but DescribeAlarms(AlarmNames=[<that name>], AlarmTypes=["MetricAlarm"]) returns an EMPTY MetricAlarms list. The DescribeAlarms handler itself filters correctly over its store, so the alarm was never written to the CloudWatch alarm store by the resource provisioner (Hypothesis: the AWS::CloudWatch::Alarm resource handler records a stack resource without calling through to CloudWatch's PutMetricAlarm state).
Why: every client that deploys alarms as infrastructure (CFN/CDK/SAM) then reads or acts on them - DescribeAlarms, SetAlarmState, alarm actions, dashboards, incident tooling - sees no alarm at all, and the only sign of trouble is an empty list rather than an error.
Fix: the CloudFormation AWS::CloudWatch::Alarm resource handler's propagation into CloudWatch's alarm state.
Similar open issues under other service headings: 20261109T185037Z-cloudformation-cloudwatch-sns-sqs-an-aws-cloudwatch-alarm-created-by-a-cdk.md and 20261029T182452Z-cloudwatch-sns-alarm-actions-cfn-alarms-are-never.md - neither carries these checks' replay keys.
Traces: e2e-diagnose-cloudwatch-latency-breach-from-alarm-config-lifecycle (phase 1/6 deploy), and the same alarm estate underlies the second check.

Cause 2 of 2: a Fargate task accepted by RunTask never executes its container, so nothing the container would emit ever appears.
AWS: Smithy (ecs) RunTask returns tasks (and failures); a FARGATE task with an awsvpc network configuration starts its container definition, which here loops reading two SSM parameters and calling cloudwatch:PutMetricData. On AWS both the control task (whose task role allows ssm:GetParameter) and the negative task (whose role does not) publish their own counters - FlagReads{Result=ok} from the allowed task and FlagReads{Result=denied} from the refused one - so GetMetricStatistics over a 60 s period returns SampleCount >= 3 for each within the task deadline.
Emulator: RunTask answers with one task and no failures for both task definitions, but neither container ever runs: GetMetricStatistics returns no datapoints for either series, (0, 0), for the full 420 s window. Nothing the container does is observable (no metrics, and by the same token no awslogs output), so a differential that needs BOTH a positive and a negative task cannot even establish its positive control.
Why: any workload whose behaviour lives inside a container - publishing metrics or traces, reading configuration, writing to a queue or bucket - produces nothing, while the ECS control plane reports a healthy task. A client cannot distinguish "my container failed" from "containers do not run", and every permission differential built on a running task is unobservable.
Fix: the ECS task runtime that is supposed to execute a task definition's container definitions (the RunTask/task-state execution path), not the RunTask request handler.
A similar open issue under another service heading: 20261104T060239Z-cloudwatch-ecr-ecs-fargate-tasks-never-leave-running.md - it does not carry this check's replay key.
Traces: fargate-task-role-without-ssm-get-parameter-is-denied-the-latency-flag.

## Evidence
Cause 1 - cloudwatch__ecs__ssm.e2e-diagnose-cloudwatch-latency-breach-from-alarm-config-lifecycle:
AssertionError: [phase 1/6 deploy] alarm 'sim2real-flint-<token>-service-latency' should be described exactly once, got []
Read: cw.describe_alarms(AlarmNames=[<name>], AlarmTypes=["MetricAlarm"])["MetricAlarms"]
Expected (AWS): exactly one MetricAlarm echoing the template (Namespace, MetricName 'Time', ExtendedStatistic 'p90', Period, EvaluationPeriods 3, Threshold, ComparisonOperator 'GreaterThanThreshold', TreatMissingData 'notBreaching', Unit 'Milliseconds').
Actual (emulator): [] - although the stack is CREATE_COMPLETE and its ServiceLatencyAlarmName output resolves to that name.

Cause 2 - cloudwatch__ecs__ssm.fargate-task-role-without-ssm-get-parameter-is-denied-the-latency-flag:
AssertionError: the control reading the flag and the denied task reporting refusals not met within 420s; last (0, 0)
RunTask answered one task with no failures for both the allowed and the denied task definition; GetMetricStatistics (Period 60, Statistics SampleCount/Minimum) over FlagReads{Result=ok} and FlagReads{Result=denied} returned no datapoints at all.
Expected (AWS): SampleCount >= 3 on each series, and APILatency from the allowed task.

## Steps to Reproduce
Cause 1:
1. CreateStack with an AWS::CloudWatch::Alarm (ExtendedStatistic p90 over a custom namespace/metric with two dimensions) and an Output that Refs it. Wait for CREATE_COMPLETE.
2. Call cloudwatch:DescribeAlarms with AlarmNames=[<the Output value>] and AlarmTypes=["MetricAlarm"].
3. Observe MetricAlarms == [].

Cause 2:
1. RegisterTaskDefinition for a FARGATE task whose container loops calling ssm:GetParameter and cloudwatch:PutMetricData, with a task role that allows cloudwatch:PutMetricData (one definition also allowing ssm:GetParameter, one not).
2. ecs:RunTask both definitions in a cluster with an awsvpc network configuration and assignPublicIp ENABLED; both succeed with no failures.
3. Poll cloudwatch:GetMetricStatistics for the counters the container publishes. Observe no datapoints ever appear.
