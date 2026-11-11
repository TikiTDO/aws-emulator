Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__logs.ecr-pull-through-cache-image-pulled-by-ecs-task ecr__ecs__logs.ecs-exec-role-without-ecr-auth-cannot-pull
Issue-Key: ecr__ecs__logs.ecr-pull-through-cache-image-pulled-by-ecs-task|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: ecr__ecs__logs.ecs-exec-role-without-ecr-auth-cannot-pull|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ecr__ecs__logs: an ECS task neither pulls its ECR image nor settles, so neither a pull-through-cache pull nor a missing-ecr-auth refusal is ever observable (1 cause)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
One net-new root cause, grounded in failing behavioral checks under the suite heading `ecr__ecs__logs`.

Cause 1 of 1: the ECS image-pull step is never performed, and the task never reaches a terminal state.
AWS: Smithy: ecs#Task carries `pullStartedAt`/`pullStoppedAt` ("the time when the container image pull began/completed"), `lastStatus` (Task Lifecycle) and `stopCode`/`stoppedReason`. A task referencing an ECR pull-through-cache repository causes the upstream image to be imported into the cache repository on first pull and then runs (its awslogs stream carries the container output); a task whose execution role lacks ecr:GetAuthorizationToken/BatchGetImage stops with stopCode=TaskFailedToStart and a CannotPullContainerError stoppedReason and writes no log stream. The exact reason strings are recorded behavior only; the requirement is that the pull is attempted and the task becomes terminal.
Emulator: both checks exhaust their poll budget ("Check timed out after 60.0s"): the task never stops, no image is imported into the pull-through-cache repository, and the execution role's ECR permissions are never evaluated - so the allowed pull and the denied pull are indistinguishable, and neither produces log events.
Why: a client cannot verify that an ECS workload can actually obtain its image: cache warm-up assertions, IAM-boundary tests on the execution role and log-based verification of the running container all hang instead of returning a verdict.
Fix: the ECS task state machine behind RunTask/DescribeTasks must perform image resolution against ECR under the execution role (including pull-through-cache import) and drive the task to a terminal state with stopCode/stoppedReason (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src, with the ECR side in services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src).
Traces: ecr__ecs__logs.ecr-pull-through-cache-image-pulled-by-ecs-task, ecr__ecs__logs.ecs-exec-role-without-ecr-auth-cannot-pull.

## Evidence
Both checks fail with "Check timed out after 60.0s". Expected: for the pull-through-cache check, the referenced upstream image appears in the cache repository (DescribeImages) and the task runs to completion with its log events; for the exec-role check, the task stops with stopCode=TaskFailedToStart and a CannotPullContainerError stoppedReason and no log stream. Actual: the task stays non-terminal, no cache import happens, and no verdict is reachable.

## Steps to Reproduce
1. Create an ECR pull-through cache rule, register a FARGATE task definition referencing <account>.dkr.ecr.<region>.amazonaws.com/<prefix>/<upstream-image>, RunTask and poll DescribeTasks/DescribeImages - the image is never imported and the task never settles.
2. Register the same kind of task definition with an execution role that has no ECR permissions, RunTask and poll DescribeTasks - the task never stops with CannotPullContainerError.

## Resolution
The image pull is now performed at launch as the task's execution role: Amazon ECR decides it
(`ecr:GetAuthorizationToken` on the registry, `ecr:BatchGetImage` on the repository, over the
role's identity documents, its permissions boundary and the repository's own policy), resolves
the reference, and caches the upstream image on the first pull of a reference under a pull
through cache rule's prefix; the task records `pullStartedAt`/`pullStoppedAt`, a task whose
image is not served stops with `TaskFailedToStart` and a `CannotPullContainerError`
`stoppedReason`, and a task placed on its own reaches the terminal `STOPPED` status with
`EssentialContainerExited` once the work it was placed to do is finished.

Known divergences that remain, each needing a capability this emulator does not have:

* a CACHED image is the registry's own record of what was cached (an OCI manifest over one
  configuration document the repository holds, every digest and size computed over those
  bytes). No upstream registry is contacted — the emulator makes no outbound connection — so a
  client that compares the cached digest or layer list against the real upstream registry will
  diverge.
* a container reports NO `exitCode`, and its stdout reaches no awslogs stream: this emulator
  starts no container process, so nothing observed an exit status and nothing produced the
  output a log event would carry. The awslogs half is tracked as cause 1 of
  `issues/things-that-do-not-work/20261104T060239Z-cloudwatch-ecr-ecs-logs-tasks-never-settle-and.md`,
  and both recorded units of this issue still wait on it.
