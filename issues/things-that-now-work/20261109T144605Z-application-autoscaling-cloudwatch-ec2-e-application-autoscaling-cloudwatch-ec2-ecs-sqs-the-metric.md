Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__cloudwatch__ec2__ecs__sqs.metrics-drive-autoscaling-unfed-alarm-stays-insufficient-and-never-scales
Issue-Key: application-autoscaling__cloudwatch__ec2__ecs__sqs.metrics-drive-autoscaling-unfed-alarm-stays-insufficient-and-never-scales|_poll_until#29|Timeout|time.sleep(interval_s)
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# application-autoscaling__cloudwatch__ec2__ecs__sqs: the metric-driven Application Auto Scaling scenario never reaches an observable step (1 cause)

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterScalableTarget

## Description
Cause 1 of 1: a metric-driven Application Auto Scaling scenario over an ECS service never reaches any observable step, so the suite burns its entire 900s budget without a verdict.

AWS: recorded behavior only for the end-to-end timing. On real AWS the check registers an ecs:service:DesiredCount scalable target, attaches a step-scaling policy, drives one CloudWatch alarm with PutMetricData while leaving a twin alarm unfed, and observes that (a) the fed alarm transitions to ALARM and Application Auto Scaling scales the ECS service out, and (b) the unfed alarm stays INSUFFICIENT_DATA and triggers no scaling activity at all. Each of those is observable within the check's own sub-budgets (metric arrival 900s, scale-out 600s, task RUNNING 300s). The Smithy model for application-autoscaling binds RegisterScalableTarget/PutScalingPolicy/DescribeScalingActivities, and for cloudwatch PutMetricData/DescribeAlarms; none of them is optional for this path.

Emulator: the check is killed with 'Check timed out after 900.0s' before any of its legs produce a value. Nothing in the chain — EC2 capacity for the cluster, the ECS service reaching a RUNNING task, the CloudWatch alarm leaving INSUFFICIENT_DATA, or Application Auto Scaling recording a scaling activity — advances far enough for the check to assert on it, so the emulator never even gets to the interesting negative (an unfed alarm must NOT scale).

Why: a client that wires metric-driven autoscaling against the emulator cannot distinguish 'correctly did not scale' from 'nothing works'. Autoscaling behaviour is unverifiable: a scaling policy can neither be shown to fire nor shown to stay quiet, so capacity-management code written against the emulator is untested in both directions.

Fix: the convergence loop that has to make this scenario progress — ECS task placement/lifecycle so a service reaches runningCount>0, CloudWatch alarm evaluation so an alarm leaves INSUFFICIENT_DATA on ingested datapoints, and the Application Auto Scaling step-scaling executor that records a ScalingActivity and calls ECS UpdateService. At minimum the emulator must reach one terminal state per leg within the check's sub-budgets rather than hanging.

Traces: application-autoscaling__cloudwatch__ec2__ecs__sqs.metrics-drive-autoscaling-unfed-alarm-stays-insufficient-and-never-scales.

## Evidence
Expected: both legs reach a terminal observation — the fed alarm goes ALARM and the ECS service's desiredCount rises; the unfed alarm stays INSUFFICIENT_DATA with no scaling activity — each inside the check's own sub-budgets.
Actual: `Error: Check timed out after 900.0s` with no leg ever producing a value.

## Steps to Reproduce
1. Create a VPC + public subnet and an ECS cluster with EC2 capacity, and an ECS service with desiredCount=1.
2. application-autoscaling RegisterScalableTarget on ecs:service:DesiredCount (min 1, max 6) and PutScalingPolicy with a StepScaling policy.
3. cloudwatch PutMetricData to drive one alarm above its threshold (50.0) and leave a twin alarm unfed.
4. Poll DescribeAlarms and ECS DescribeServices for the fed alarm reaching ALARM and desiredCount rising; then watch the unfed alarm for 180s.
Observed: the check never completes any leg and is killed at 900s.
