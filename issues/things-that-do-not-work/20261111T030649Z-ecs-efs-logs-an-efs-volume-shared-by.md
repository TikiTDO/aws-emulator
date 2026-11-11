Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__efs__logs.ecs-efs-volume-shared-across-two-fargate-tasks-and-readonly-mount-write-denied
Issue-Key: ecs__efs__logs.ecs-efs-volume-shared-across-two-fargate-tasks-and-readonly-mount-write-denied|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Commit: 6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf

# ecs__efs__logs: an EFS volume shared by two Fargate tasks never becomes observable — neither the shared write/read nor the read-only mount refusal happens

> _Note: this issue was identified against the code base at commit `6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
One net-new root cause. The check is not marked ALREADY FILED and is not OWNED.

Cause 1 of 1: a task with an efsVolumeConfiguration volume never produces the container-side evidence of the mount, so neither shared access nor readOnly enforcement happens.
AWS: two Fargate tasks that mount the same EFS file system (task definition volumes[].efsVolumeConfiguration naming the file system id and access point) see one shared filesystem: a file written by task A is read by task B, and a container whose mountPoints[].readOnly is true is refused the write with EROFS ("Read-only file system") while the read-write sibling's write succeeds. The containers report that outcome on stdout, which the awslogs driver delivers to the configured log group. Recorded behavior for the data plane; Smithy models the plumbing (ecs EFSVolumeConfiguration, MountPoint.readOnly).
Emulator: the check is killed with `Error: Check timed out after 60.0s` — no container write, read or refusal is ever observable, so neither the sharing nor the readOnly guarantee can be established. Hypothesis: the task's container process does not run (sibling ECS checks in this suite report exitCode 127 and stoppedReason 'Essential container in task exited' for the same busybox images), so no EFS mount is exercised and nothing reaches the log group.
Why: readOnly on a mount point is a containment guarantee and a shared EFS volume is a correctness guarantee. A client gets neither enforcement nor evidence: a task that should have been refused a write is reported as having run, and a shared volume that should have carried data between tasks is silent.
Fix: the ECS task-execution path — it must actually run the container process, attach the declared efsVolumeConfiguration volume into each container at its mountPoints, honour mountPoints[].readOnly, and deliver the container's stdout through the configured awslogs driver.
Traces: ecs__efs__logs.ecs-efs-volume-shared-across-two-fargate-tasks-and-readonly-mount-write-denied.

## Evidence
Cause 1 — Expected: task A's write to the shared EFS volume is read back by task B, and the readOnly mount's write is refused with EROFS while the read-write sibling's write succeeds — all three outcomes readable from the containers' awslogs streams. Actual: `Error: Check timed out after 60.0s`; no container output, no write, no read, no refusal.

## Steps to Reproduce
1. Create an EFS file system (and access point) reachable from the task subnets.
2. Register a task definition with volumes[].efsVolumeConfiguration naming it, mounted into one container read-write and (in the second task) read-only; awslogs to a log group.
3. RunTask twice on Fargate and poll the log group for the write/read/EROFS lines.
Observed: nothing is delivered and the check is killed at its 60 s budget.
