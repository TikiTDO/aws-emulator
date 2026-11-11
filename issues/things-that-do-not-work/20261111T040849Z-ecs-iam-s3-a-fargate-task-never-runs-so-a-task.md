Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__iam__s3.fargate-task-role-missing-s3-putobject-grant-is-denied-then-repaired
Issue-Key: ecs__iam__s3.fargate-task-role-missing-s3-putobject-grant-is-denied-then-repaired|_wait_log_line#24|Timeout|time.sleep(3)
Issue-Commit: c0ced5d18fa4eb624de2042a998eca41dbf9ea10

# A Fargate task never runs, so a task role's denied-then-repaired S3 write is never observable

> _Note: this issue was identified against the code base at commit `c0ced5d18fa4eb624de2042a998eca41dbf9ea10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
One net-new root cause.

Cause 1 of 1: an ECS task launched on Fargate never executes its container, so the S3 PutObject the task performs under its task role is never attempted — neither the denial while the grant is missing nor the success after the grant is added can be seen.
AWS: recorded behavior only for task execution (Smithy models RunTask/DescribeTasks but not the container run). The recording shows the task reaching RUNNING and then STOPPED with exit code 0, the task-role-signed PutObject refused with AccessDenied while the role lacks s3:PutObject, and the same task succeeding once the grant is attached.
Emulator: 'Error: Check timed out after 60.0s' — the task never reaches a state the check can grade and no object or denial is ever produced.
Why: a client cannot run container work at all, so task-role least-privilege (the main reason to separate task role from execution role) cannot be exercised and a missing grant produces no diagnosis.
Fix: the ECS task lifecycle/container execution path — RunTask must actually run the task definition's container with the task role's credentials and drive the task through RUNNING to STOPPED.
Traces: ecs__iam__s3.fargate-task-role-missing-s3-putobject-grant-is-denied-then-repaired.

## Evidence
Check ecs__iam__s3.fargate-task-role-missing-s3-putobject-grant-is-denied-then-repaired [mismatch]:
Error: Check timed out after 60.0s

Expected: the task runs, its PutObject is refused with AccessDenied while the task role lacks s3:PutObject, and succeeds after the grant is added.
Actual: the task never runs; the check exhausts its budget.

## Steps to Reproduce
1. Create a cluster, a task role WITHOUT s3:PutObject, an execution role and a task definition whose container writes an object to a bucket.
2. RunTask with launchType=FARGATE and poll DescribeTasks.
3. The task never reaches RUNNING/STOPPED with a container result; no object and no AccessDenied are produced.
