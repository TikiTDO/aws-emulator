Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs__logs.cli-ecs-task-log-delivery-blocked-without-permission-rejected cloudwatch__ecr__ecs__logs.deploy-stack-fails-without-log-group-resource-policy
Issue-Key: cloudwatch__ecr__ecs__logs.cli-ecs-task-log-delivery-blocked-without-permission-rejected|run#32|AssertionError|assert "ResourceInitializationError" in stopped_reason, (
Issue-Key: cloudwatch__ecr__ecs__logs.deploy-stack-fails-without-log-group-resource-policy|run#15|AssertionError|assert "ResourceInitializationError" in negative_stop_reason, (
Issue-Commit: 6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf

# cloudwatch__ecr__ecs__logs: an ECS task whose execution role may not write to its awslogs log group is not failed at start with ResourceInitializationError

> _Note: this issue was identified against the code base at commit `6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
One net-new root cause covering two checks. Neither is marked ALREADY FILED nor OWNED; the open issues for this family (20261109T141609Z-cloudwatch-ecr-ecs-logs-3-net-new-causes.md, 20261104T060239Z-cloudwatch-ecr-ecs-logs-tasks-never-settle-and.md) do not carry these replay keys.

Cause 1 of 1: the awslogs log-driver initialization is never authorized against the task's execution role, so a task that AWS refuses to start is reported as a task that started and whose container exited.
AWS: a Fargate/ECS task whose logConfiguration is awslogs fails BEFORE any container runs when the execution role may not create the stream or put events into the named group (or when no resource policy grants it). DescribeTasks reports lastStatus STOPPED with stopCode TaskFailedToStart and a stoppedReason beginning with the fixed prefix `ResourceInitializationError:` — AWS words it `ResourceInitializationError: failed to validate logger args: create stream has been retried N times: failed to create Cloudwatch log stream: AccessDeniedException: User: <execution-role session arn> is not authorized to perform: logs:CreateLogStream on resource: <log-group/log-stream arn> ...` where the role arn, action, resource arn and retry count come from the request and the emulator's IAM/Logs state and the rest is fixed text. The containers list carries no exitCode, and the log group stays empty. Smithy: ecs TaskStopCode enum = TaskFailedToStart | EssentialContainerExited | UserInitiated | ServiceSchedulerInitiated | SpotInterruption | TerminationNotice | InfrastructureHealth; the ResourceInitializationError wording is AWS-documented behavior (ECS stopped-task troubleshooting), not modelled.
Emulator: the task is admitted, reaches a running state and then stops with stopCode EssentialContainerExited and stoppedReason 'Essential container in task exited' — no ResourceInitializationError prefix and no TaskFailedToStart anywhere, in both the CLI-driven negative and the CloudFormation-deployed missing-resource-policy negative.
Why: the negative is indistinguishable from a healthy task whose process simply finished. A client cannot tell "ECS refused to start my task because its execution role cannot log" from "my container ran and exited", so a misconfigured execution role is reported as a successful run with no logs — exactly the failure the stopCode/stoppedReason contract exists to surface. Automation that branches on stopCode == 'TaskFailedToStart' or on the ResourceInitializationError prefix never fires.
Fix: the ECS task-start path (RunTask / the service scheduler's task placement) must validate the task definition's awslogs logConfiguration against the execution role and the destination log group's resource policy before starting any container, and set lastStatus STOPPED, stopCode TaskFailedToStart and the ResourceInitializationError stoppedReason reported by DescribeTasks when that validation fails.
Traces: cloudwatch__ecr__ecs__logs.cli-ecs-task-log-delivery-blocked-without-permission-rejected, cloudwatch__ecr__ecs__logs.deploy-stack-fails-without-log-group-resource-policy.

## Evidence
Cause 1 — Expected: DescribeTasks reports stopCode 'TaskFailedToStart' and a stoppedReason starting with 'ResourceInitializationError:' naming the denied logs action and the log-group resource; the group stays empty. Actual: `AssertionError: expected ResourceInitializationError due to missing logs permission, got: 'Essential container in task exited'` and `AssertionError: execution role with no logs permissions should stop with ResourceInitializationError, got stop reason 'Essential container in task exited'` (stopCode EssentialContainerExited).

## Steps to Reproduce
1. Create a log group; register a task definition whose container uses the awslogs driver pointed at it.
2. Give the task's execution role NO logs permissions (and/or omit the log group's resource policy).
3. RunTask (or deploy the equivalent CloudFormation stack and let the service place a task) and poll DescribeTasks until the task is STOPPED.
Observed: stopCode EssentialContainerExited, stoppedReason 'Essential container in task exited'. Expected: stopCode TaskFailedToStart with a stoppedReason beginning 'ResourceInitializationError:' that names the denied logs action and resource.
