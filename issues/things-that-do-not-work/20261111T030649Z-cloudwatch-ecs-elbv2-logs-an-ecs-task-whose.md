Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__elbv2__logs.ecs-log-group-name-mismatch-no-delivery cloudwatch__ecs__elbv2__logs.ecs-task-fails-log-delivery-without-logs-permission
Issue-Key: cloudwatch__ecs__elbv2__logs.ecs-log-group-name-mismatch-no-delivery|run#10|AssertionError|assert reason.startswith("ResourceInitializationError")
Issue-Key: cloudwatch__ecs__elbv2__logs.ecs-task-fails-log-delivery-without-logs-permission|run#10|AssertionError|assert reason.startswith("ResourceInitializationError"), (
Issue-Commit: 6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf

# cloudwatch__ecs__elbv2__logs: an ECS task whose awslogs destination is unwritable (wrong group name, or no logs permission) is not failed at start with ResourceInitializationError

> _Note: this issue was identified against the code base at commit `6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
One net-new root cause covering two checks. Neither is marked ALREADY FILED nor OWNED; the open issues for this family (20261108T191814Z-cloudwatch-ecs-elbv2-logs-an-alb-answers-502.md, 20261110T220525Z-cloudwatch-ecs-elbv2-logs-an-alb-answers-502-47cb37.md, 20261104T062603Z-cloudwatch-ecs-elbv2-logs-task-attachments-expose-no.md) do not carry these replay keys.

Cause 1 of 1: awslogs log-driver initialization failure is never surfaced as a start failure.
AWS: when a task's awslogs logConfiguration names a log group the execution role may not write to — or a group that does not exist and the role lacks logs:CreateLogGroup — ECS fails the task before any container runs. DescribeTasks reports lastStatus STOPPED, stopCode TaskFailedToStart, and a stoppedReason whose fixed prefix is `ResourceInitializationError:`, continuing (AWS's wording) `failed to validate logger args: create stream has been retried N times: failed to create Cloudwatch log stream: <AWS error code>: <the Logs/IAM refusal message naming the role session arn, the action logs:CreateLogStream or logs:CreateLogGroup, and the log-group resource arn>`. The role arn, action, resource arn, group name and retry count are values from the request and the emulator's IAM/Logs state; the prefix and the connecting phrases are fixed. Smithy: ecs TaskStopCode enum includes TaskFailedToStart; the ResourceInitializationError wording is AWS-documented stopped-task behavior, not modelled.
Emulator: both the log-group-name-mismatch case and the no-logs-permission case produce a task that is admitted and then stops with stopCode EssentialContainerExited and stoppedReason 'Essential container in task exited'; `reason.startswith("ResourceInitializationError")` is false.
Why: a client cannot distinguish a task ECS refused to start (because its log destination is unwritable) from a task that ran and whose essential container exited. The misconfiguration is reported as a normal run with no delivered logs, and any automation keyed on stopCode TaskFailedToStart or on the ResourceInitializationError prefix never triggers.
Fix: the ECS task-start path (RunTask and the service scheduler) must resolve and authorize the awslogs destination against the execution role before starting containers, and report lastStatus STOPPED + stopCode TaskFailedToStart + the ResourceInitializationError stoppedReason through DescribeTasks when it cannot.
Traces: cloudwatch__ecs__elbv2__logs.ecs-log-group-name-mismatch-no-delivery, cloudwatch__ecs__elbv2__logs.ecs-task-fails-log-delivery-without-logs-permission.

## Evidence
Cause 1 — Expected: `reason.startswith("ResourceInitializationError")` holds and stopCode is TaskFailedToStart. Actual: bare `AssertionError` on that assertion for the group-name-mismatch check, and `AssertionError: expected a ResourceInitializationError from the awslogs driver init, got: 'Essential container in task exited'` for the missing-permission check.

## Steps to Reproduce
1. Register a task definition whose container uses awslogs with awslogs-group set to a group that does not exist (check A) or to an existing group the execution role has no logs permissions for (check B).
2. RunTask and poll DescribeTasks until STOPPED.
Observed: stopCode EssentialContainerExited, stoppedReason 'Essential container in task exited'. Expected: stopCode TaskFailedToStart with a stoppedReason beginning 'ResourceInitializationError:'.
