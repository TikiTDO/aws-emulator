Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.essential-container-exit-stops-task-and-sidecar ecs.run-task-container-override-exit-code ecs.run-task-environment-override-injected ecs.update-task-protection-extends-existing-expiration
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.essential-container-exit-stops-task-and-sidecar|_poll_until#21|Timeout|return last
Issue-Key: ecs.run-task-container-override-exit-code|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Key: ecs.run-task-environment-override-injected|_poll_until#26|Timeout|return last
Issue-Key: ecs.update-task-protection-extends-existing-expiration|run#16|Timeout|time.sleep(2)
Issue-Commit: dff6e291e5e6489707888b0049fa49b367909ca3

# 4 net-new causes: essential-container exit, RunTask command/environment overrides and UpdateTaskProtection expiration all never resolve

> _Note: this issue was identified against the code base at commit `dff6e291e5e6489707888b0049fa49b367909ca3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Four distinct root causes in the ECS task data plane, each grounded in its own failing check. All four end with the check exhausting its budget because the state AWS reaches is never reached.

Cause 1 of 4: an essential container's exit does not stop the task (and does not stop the sidecar).
AWS: Smithy: DescribeTasks -> Task.stopCode is TaskStopCode, whose members include EssentialContainerExited; Container.exitCode carries the process exit status. The recording shows a non-essential container exiting while the task stays RUNNING, and the essential container exiting making the task STOPPED with stopCode 'EssentialContainerExited', its exitCode preserved, and the still-running non-essential sidecar stopped too.
Emulator: the task never changes state, so the poll never terminates — 'Error: Check timed out after 60.0s'.
Why: the essentiality contract is the core of multi-container task design; a client's sidecar pattern and any "task finished" wait never complete and stopCode/exitCode are unavailable for diagnosis.
Fix: the ECS task lifecycle/container-state component — container exit must be observed, essentiality evaluated, the task stopped with stopCode EssentialContainerExited and sibling containers stopped.
Failing check: ecs.essential-container-exit-stops-task-and-sidecar.

Cause 2 of 4: RunTask containerOverrides.command does not replace the task definition command and no exit code is ever reported.
AWS: Smithy: RunTask.overrides -> TaskOverride.containerOverrides -> ContainerOverride.command — "The command to send to the container that overrides the default command from the Docker image or the task definition." The recording shows DescribeTasks reporting the override's exit code (42), not the definition's 0, once the task reaches STOPPED.
Emulator: the task never reaches a stopped state with an exit code; 'Error: Check timed out after 60.0s'.
Why: a client that parameterises one task definition per invocation via command overrides gets neither the overridden behaviour nor any exit code, so batch/one-shot workloads cannot be driven or graded.
Fix: the RunTask handler's override merging plus the task runtime that must execute the merged command and record Container.exitCode on exit.
Failing check: ecs.run-task-container-override-exit-code.

Cause 3 of 4: RunTask containerOverrides.environment does not reach the container process.
AWS: Smithy: ContainerOverride.environment (sensitive) — environment variables sent to the container, overriding the task definition's values. The recording shows the override value winning (exit code 17, not the definition's 1), and an override naming a container absent from the task definition being rejected with InvalidParameterException (400) and launching no task.
Emulator: the check never gets past its waits — 'Error: Check timed out after 60.0s'; the overridden environment is never observable because the container's process outcome never materialises.
Why: environment overrides are how one definition is reused per job; silently losing them (or never running the process) makes every parameterised run indistinguishable from the default.
Fix: the RunTask handler's containerOverrides.environment merge (including the InvalidParameterException refusal for an unknown container name) and the task runtime that injects it into the container process.
Failing check: ecs.run-task-environment-override-injected.

Cause 4 of 4: UpdateTaskProtection re-applied with the same expiresInMinutes does not move expirationDate forward.
AWS: Smithy: UpdateTaskProtection(cluster, tasks, protectionEnabled, expiresInMinutes) -> ProtectedTask.expirationDate, "The epoch time when protection for the task will expire"; the documented rule is that expiration is reset from NOW on each call, so a second identical call made later yields an expirationDate later by about the elapsed time, and GetTaskProtection reports that date.
Emulator: the check never completes — 'Error: Check timed out after 60.0s'; it does not get a protected task whose expirationDate advances (the service task it needs never settles into a state where protection can be read back).
Why: scale-in protection renewal is the mechanism a long job uses to keep itself alive; if expirationDate does not advance, protection lapses mid-job and the task is killed by scale-in.
Fix: the UpdateTaskProtection handler and its task-protection state (expirationDate recomputed from the call time, surfaced by GetTaskProtection), and the service task lifecycle that must produce a RUNNING task to protect.
Failing check: ecs.update-task-protection-extends-existing-expiration.

Traces: the four check ids above. Open ECS issues such as 20261104T062603Z-ecs-containers-never-execute-no-in-container-behaviour.md and 20261104T062603Z-ecs-runtask-clienttoken-is-ignored-capacity-provider-base.md describe neighbouring causes but carry none of these four replay keys.

## Evidence
Cause 1 — Expected: DescribeTasks reports lastStatus STOPPED, stopCode 'EssentialContainerExited', the essential container's exitCode preserved and the sidecar stopped. Actual: no state change; 'Error: Check timed out after 60.0s'.
Cause 2 — Expected: DescribeTasks reports exitCode 42 (the containerOverrides command), not the definition's 0. Actual: no exit code ever reported; 'Error: Check timed out after 60.0s'.
Cause 3 — Expected: the overridden SIM2REAL_EXIT yields exit code 17 (not the definition's 1), and an override naming 'not-in-this-task-definition' is refused with InvalidParameterException (HTTP 400) launching no task. Actual: 'Error: Check timed out after 60.0s'.
Cause 4 — Expected: a second UpdateTaskProtection with the same expiresInMinutes returns a ProtectedTask.expirationDate later by about the elapsed time, echoed by GetTaskProtection. Actual: 'Error: Check timed out after 60.0s'.

## Steps to Reproduce
Cause 1: RegisterTaskDefinition with an essential container that exits non-zero after a delay and a non-essential sleeper; RunTask; poll DescribeTasks for STOPPED + stopCode EssentialContainerExited.
Cause 2: RunTask with overrides={containerOverrides:[{name:'app', command:['sh','-c','exit 42']}]}; poll DescribeTasks for containers[0].exitCode == 42.
Cause 3: RegisterTaskDefinition with environment SIM2REAL_EXIT=1 and command 'exit ${SIM2REAL_EXIT}'; RunTask with containerOverrides environment SIM2REAL_EXIT=17 and expect exitCode 17; separately RunTask with an override naming an unknown container and expect InvalidParameterException with no task created.
Cause 4: CreateService desiredCount=1, wait for the RUNNING task, UpdateTaskProtection(protectionEnabled=true, expiresInMinutes=N), wait, call it again with the same N and compare ProtectedTask.expirationDate / GetTaskProtection.
