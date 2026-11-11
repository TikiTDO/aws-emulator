Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__events__logs__sqs.ecs-container-oom-kill-reason-matches-eventbridge-task-state-change-detail
Issue-Key: ecs__events__logs__sqs.ecs-container-oom-kill-reason-matches-eventbridge-task-state-change-detail|run#5|AssertionError|assert vc.get("exitCode") == 137, (
Issue-Commit: 6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf

# ecs__events__logs__sqs: an OOM-killed ECS container reports exitCode 127 with no OutOfMemoryError reason, so the EventBridge Task State Change detail never carries the kill reason

> _Note: this issue was identified against the code base at commit `6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
One net-new root cause. The check is not marked ALREADY FILED and is not OWNED; the open issue 20261105T143253Z-ecs-events-logs-sqs-an-eventbridge-ecs-runtask.md covers the sibling check in this family but not this replay key.

Cause 1 of 1: a container killed for exceeding its memory limit is not reported as OOM-killed, and in fact its command never ran.
AWS: a container whose process exceeds the container-level `memory` hard limit is killed by the kernel. DescribeTasks reports that container with exitCode 137 (128 + SIGKILL) and `reason` = the fixed text `OutOfMemoryError: Container killed due to memory usage`, with the task's stopCode EssentialContainerExited and stoppedReason `Essential container in task exited`; the same container-level exitCode and reason appear verbatim in the `detail.containers[]` of the ECS "Task State Change" event delivered to EventBridge. Smithy: ecs Container.exitCode / Container.reason; TaskStopCode enum includes EssentialContainerExited. The 137/OutOfMemoryError pairing is AWS-documented stopped-task behavior.
Emulator: the container is reported with exitCode 127 (the shell's "command not found"), reason None, and stoppedReason 'Essential container in task exited'. 127 with no reason shows the container's command never executed at all, so the memory limit was never reached and there is nothing for the EventBridge detail to echo.
Why: a client diagnosing a crash loop keys on exitCode 137 and on the OutOfMemoryError reason to tell "the kernel killed my container for memory" from "my process exited by itself". Here every container exits 127 with no reason, so OOM is invisible, the sizing decision it drives is impossible, and the EventBridge consumer that routes on `detail.containers[].reason` never matches.
Fix: the ECS task-execution path must actually run the container's command, enforce the container-level memory hard limit, and set Container.exitCode 137 with Container.reason 'OutOfMemoryError: Container killed due to memory usage' on an OOM kill — and the ECS "Task State Change" EventBridge publisher must copy those container fields into detail.containers[].
Traces: ecs__events__logs__sqs.ecs-container-oom-kill-reason-matches-eventbridge-task-state-change-detail.

## Evidence
Cause 1 — Expected: DescribeTasks reports the OOM-killed container with exitCode 137 and reason 'OutOfMemoryError: Container killed due to memory usage', and the EventBridge 'ECS Task State Change' detail carries the same values. Actual: `AssertionError: an OOM-killed container must report exit code 137; got {... 'name': 'app', 'image': 'public.ecr.aws/docker/library/busybox@sha256:...', 'lastStatus': 'STOPPED', 'exitCode': 127, 'healthStatus': 'UNKNOWN', ... 'memory': '512'} stoppedReason='Essential container in task exited'` — exitCode 127, no `reason` member at all.

## Steps to Reproduce
1. Register a task definition with one essential container (busybox) given a small container-level `memory` hard limit and a command that allocates past it; awslogs to a log group.
2. Wire an EventBridge rule for 'ECS Task State Change' to an SQS queue.
3. RunTask, poll DescribeTasks until STOPPED, and read the queued event.
Observed: container exitCode 127, reason absent, stoppedReason 'Essential container in task exited'. Expected: exitCode 137 with reason 'OutOfMemoryError: Container killed due to memory usage', mirrored in detail.containers[].
