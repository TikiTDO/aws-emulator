Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__kms__logs__ssm.xsvc-ecs-3-secret-injection-no-kms-decrypt-rejected ecs__kms__logs__ssm.xsvc-ecs-3-secret-injection-task-role-grants-rejected ecs__kms__logs__ssm.xsvc-ecs-3-secret-injection-cmk-disabled-rejected
Issue-Key: ecs__kms__logs__ssm.xsvc-ecs-3-secret-injection-no-kms-decrypt-rejected|run#17|AssertionError|assert bad.get("stopCode") == "TaskFailedToStart", (
Issue-Key: ecs__kms__logs__ssm.xsvc-ecs-3-secret-injection-task-role-grants-rejected|run#20|AssertionError|assert bad.get("stopCode") == "TaskFailedToStart", (
Issue-Key: ecs__kms__logs__ssm.xsvc-ecs-3-secret-injection-cmk-disabled-rejected|run#23|AssertionError|assert t.get("stopCode") == "TaskFailedToStart", (
Issue-Commit: 6a777fbd6800e714176d50f5d7d416617b079449

# ecs__kms__logs__ssm: container `secrets` injection is never authorized — a task whose execution role lacks kms:Decrypt (or whose CMK is disabled) starts and runs instead of stopCode TaskFailedToStart

> _Note: this issue was identified against the code base at commit `6a777fbd6800e714176d50f5d7d416617b079449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 1: ECS resolves a container definition's `secrets` (an SSM SecureString ARN) without authorizing the resolution against the EXECUTION role or checking the SecureString's CMK state. Every task starts, runs its container and ends EssentialContainerExited, so no task is ever failed at start.

AWS: before a task's containers run, the ECS agent resolves each container's `secrets` using the task definition's executionRoleArn, calling ssm:GetParameters(WithDecryption) and, for a SecureString under a customer CMK, kms:Decrypt on that key. If the execution role lacks kms:Decrypt, or the grants sit on the TASK role instead of the execution role, or the CMK is Disabled, the resolution fails and the task never starts: DescribeTasks reports lastStatus STOPPED with stopCode "TaskFailedToStart" (Smithy: ecs TaskStopCode enum = TaskFailedToStart | EssentialContainerExited | UserInitiated | ServiceSchedulerInitiated | SpotInterruption | TerminationNotice | InfrastructureHealth) and a ResourceInitializationError stoppedReason naming the failing fetch and the EXECUTION role (never the task role), with no container ever running and nothing written to the awslogs stream. Recorded behavior: the positive control — the same task definition whose execution role carries ssm:GetParameters + kms:Decrypt, with the CMK Enabled — starts and prints the exact plaintext.

Emulator: all three negative tasks are launched and run. DescribeTasks reports stopCode='EssentialContainerExited' with reason='Essential container in task exited' — i.e. the secret was injected (or the container ran regardless) with no authorization or key-state gate, and the negative and positive cases are indistinguishable.

Why: a client cannot test or trust least-privilege secret delivery. An execution role missing kms:Decrypt, grants misplaced on the task role, and a disabled CMK all look like a healthy task that ran to completion, so a misconfiguration that would never start on AWS passes locally; and the secret value is treated as readable when AWS would refuse it.

Fix: the ECS task start path — the component that resolves container `secrets`/`repositoryCredentials` for a task (it must perform the ssm:GetParameters + kms:Decrypt resolution as the execution role and honour KMS key state) and the task state machine that records stopCode/stoppedReason, which must produce TaskFailedToStart + a ResourceInitializationError reason instead of running the container.

Traces: ecs__kms__logs__ssm.xsvc-ecs-3-secret-injection-no-kms-decrypt-rejected, ecs__kms__logs__ssm.xsvc-ecs-3-secret-injection-task-role-grants-rejected, ecs__kms__logs__ssm.xsvc-ecs-3-secret-injection-cmk-disabled-rejected.

## Evidence
All three checks assert the same shape on DescribeTasks of the negative task and fail identically:

1) xsvc-ecs-3-secret-injection-no-kms-decrypt-rejected (execution role has ssm:GetParameters but NOT kms:Decrypt on the CMK):
   Expected: stopCode == 'TaskFailedToStart' (ResourceInitializationError reason naming the execution role).
   Actual: AssertionError: a task whose execution role lacks kms:Decrypt was not failed at start: stopCode='EssentialContainerExited' reason='Essential container in task exited'

2) xsvc-ecs-3-secret-injection-task-role-grants-rejected (ssm:GetParameters + kms:Decrypt on the TASK role; execution role has logs only):
   Expected: stopCode == 'TaskFailedToStart'.
   Actual: AssertionError: a task whose grants sit on the TASK role was not failed at start: stopCode='EssentialContainerExited' reason='Essential container in task exited'

3) xsvc-ecs-3-secret-injection-cmk-disabled-rejected (CMK disabled; DisableKey confirmed via DescribeKey KeyState=='Disabled' before launch; tasks both with and without kms:Decrypt on the execution role):
   Expected: stopCode == 'TaskFailedToStart' and nothing logged.
   Actual: AssertionError: task A (no kms:Decrypt, key disabled) was not failed at start: stopCode='EssentialContainerExited' reason='Essential container in task exited'

The fixed text AWS uses for the stop is the stopCode value "TaskFailedToStart"; the stoppedReason is a ResourceInitializationError sentence whose variable parts are the failing secret's parameter ARN, the CMK ARN and the execution-role ARN from the task definition and request.

## Steps to Reproduce
1. CreateKey (symmetric CMK); PutParameter(Type=SecureString, KeyId=<cmk>) with a known plaintext.
2. Create two IAM roles trusted by ecs-tasks.amazonaws.com: role_bad = logs + ssm:GetParameters on the parameter ARN (no kms:Decrypt), role_ok = the same plus kms:Decrypt on the CMK.
3. RegisterTaskDefinition twice (Fargate, awsvpc, awslogs to a log group) with a container whose `secrets` is [{name: SECRET, valueFrom: <parameter ARN>}] and which echoes $SECRET; one with executionRoleArn=role_bad, one with role_ok.
4. RunTask both; poll DescribeTasks until each reaches STOPPED.
   -> AWS: the role_bad task has stopCode 'TaskFailedToStart' with a ResourceInitializationError reason naming the execution role and writes no log events; the role_ok task runs and logs the plaintext.
   -> Emulator: both report stopCode 'EssentialContainerExited', reason 'Essential container in task exited'.
5. Variant A: put ssm:GetParameters + kms:Decrypt on the TASK role and leave only logs on the execution role — AWS still fails the task at start.
6. Variant B: DisableKey the CMK and RunTask with either role — AWS fails the task at start with or without kms:Decrypt; after EnableKey the same task definition starts and prints the plaintext.
