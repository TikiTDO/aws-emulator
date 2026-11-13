Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__elbv2__logs.deploy-ecs-task-logs-flow-to-cloudwatch cloudwatch__ecs__elbv2__logs.deploy-stack-without-log-permission-no-log-group-created
Issue-Key: cloudwatch__ecs__elbv2__logs.deploy-ecs-task-logs-flow-to-cloudwatch|run#18|AssertionError|assert container.get("exitCode") == 0, f"container did not exit cleanly: {container}"
Issue-Key: cloudwatch__ecs__elbv2__logs.deploy-stack-without-log-permission-no-log-group-created|run#24|AssertionError|assert "RUNNING" not in observed_statuses, (
Issue-Commit: f886c6313691e264df098b8c73e16bb38687852c

# cloudwatch__ecs__elbv2__logs: 2 net-new causes — a Fargate container is killed (exitCode 143) instead of exiting its own command, and a task whose execution role lacks logs:CreateLogGroup still reaches RUNNING

> _Note: this issue was identified against the code base at commit `f886c6313691e264df098b8c73e16bb38687852c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
Two net-new root causes in the ECS task-lifecycle surface. Neither failing check is marked ALREADY FILED nor OWNED in the replay report. This bundle has 2 causes, numbered below with their own failing check ids; BOTH must be closed before this issue is submitted.

RELATED OPEN ISSUES (neither carries these replay keys):
- 20261111T030649Z-cloudwatch-ecs-elbv2-logs-an-ecs-task-whose.md records that an unwritable awslogs destination is not reported with stopCode TaskFailedToStart / a stoppedReason prefixed 'ResourceInitializationError:'. Cause 2 below is the stronger, independent fact that check grades: the task must never reach RUNNING at all, and the named log group must never be created.
- 20261110T143849Z-cloudwatch-ecs-elbv2-logs-a-running-ecs-service.md and 20261104T062603Z-cloudwatch-ecs-elbv2-logs-task-attachments-expose-no.md concern service metrics and task attachments, not container exit status.

=== Cause 1 of 2: a Fargate container that runs its command to completion is reported as externally terminated (exitCode 143) ===
Failing check: cloudwatch__ecs__elbv2__logs.deploy-ecs-task-logs-flow-to-cloudwatch
AWS: recorded behavior only for the exit status (Smithy com.amazonaws.ecs — Container.exitCode is documented only as "The exit code returned from the container"). A Fargate task whose single essential container's command is a shell that prints a line and then sleeps for a bounded interval terminates of its own accord: DescribeTasks then reports lastStatus STOPPED with containers[0].exitCode equal to the command's own exit status (0 for the check's `/bin/sh -c "echo <token>; sleep 15"`). 143 is 128+SIGTERM, which AWS reports only when the task was stopped from OUTSIDE the container — StopTask, a deployment replacement, scale-in or a task-level timeout.
Emulator: the task reaches STOPPED but containers[0].exitCode is 143, with lastStatus STOPPED, healthStatus UNKNOWN and no stop reason distinguishing it from a clean exit. Hypothesis: the emulated task runtime SIGTERMs the container on a lifecycle deadline of its own rather than letting the container's command finish.
Why: exitCode is the primary success/failure signal for a run-to-completion ECS task. Every client gate keyed on `exitCode == 0` (batch-style jobs, CI tasks, CFN/CodePipeline stage gates, the check's own clean-exit precondition) inverts: a successful run is reported as a kill, and a client cannot distinguish a container that finished from one the scheduler tore down.
Fix: the ECS task-execution lifecycle and the exit-status it records into task state — a container whose command completes must be recorded with that command's exit status, and 128+signal reserved for a task actually stopped from outside; DescribeTasks then projects it unchanged.
Traces: cloudwatch__ecs__elbv2__logs.deploy-ecs-task-logs-flow-to-cloudwatch.

=== Cause 2 of 2: a task whose execution role may not create its awslogs log group is admitted and reaches RUNNING ===
Failing check: cloudwatch__ecs__elbv2__logs.deploy-stack-without-log-permission-no-log-group-created
AWS: recorded behavior only (Smithy models Task.lastStatus as a plain string and TaskStopCode includes TaskFailedToStart, but the awslogs driver-init ordering is documented ECS behavior, not modelled). The awslogs log driver is initialised BEFORE the container is started: when logConfiguration sets awslogs-create-group true for a group that does not exist and the task's execution role does not allow logs:CreateLogGroup, the task transitions PROVISIONING -> STOPPED and lastStatus is NEVER observed as RUNNING. The named log group is never created, and the stop is reported with stopCode TaskFailedToStart and a stoppedReason whose fixed prefix is 'ResourceInitializationError:' (the role session ARN, the denied action and the log-group ARN in it are values from the request and from IAM/Logs state). The contrast is the whole point: the identical task definition run with an execution role that DOES allow logs:CreateLogGroup reaches RUNNING and the group appears.
Emulator: the task is admitted and runs — DescribeTasks reports the sequence ['PROVISIONING', 'PROVISIONING', 'RUNNING', 'RUNNING', 'STOPPED'], so RUNNING is observed for a task whose log destination can never be created. The permission boundary is simply not evaluated on the task-start path.
Why: the negative half of the permission contrast is unobservable — a client can no longer tell a correctly-permissioned task from one whose execution role is missing logs:CreateLogGroup, because both reach RUNNING. IaC and least-privilege test suites that assert "this role is insufficient" pass against the emulator and fail on AWS, and automation keyed on never-RUNNING (or on TaskFailedToStart) never triggers.
Fix: the ECS task-start path (RunTask and the service scheduler) must resolve and authorize the awslogs destination against the task's EXECUTION role before any container is started, and when it cannot, stop the task from PROVISIONING without ever publishing RUNNING and without creating the log group.
Traces: cloudwatch__ecs__elbv2__logs.deploy-stack-without-log-permission-no-log-group-created.

## Evidence
Cause 1 (verdict: mismatch) — cloudwatch__ecs__elbv2__logs.deploy-ecs-task-logs-flow-to-cloudwatch:
  File "check_cloudwatch__ecs__elbv2__logs_deploy_ecs_task_logs_flow_to_cloudwatch.py", line 62, in run
  assert container.get("exitCode") == 0, f"container did not exit cleanly: {container}"
  AssertionError: container did not exit cleanly: {... 'name': 'app', 'image': 'public.ecr.aws/amazonlinux/amazonlinux:2023', 'lastStatus': 'STOPPED', 'exitCode': 143, 'networkBindings': [], 'networkInterfaces': [{'attachmentId': ..., 'privateIpv4Address': '172.31.0.5'}], 'healthStatus': 'UNKNOWN', 'managedAgents': [], 'cpu': '0'}
  (ARNs, attachment ids and the private IP in the dump are environment-dependent and not part of the requirement.)
  Task definition under test (from the stack template): ContainerDefinitions[0] = {Name: 'app', Image: 'public.ecr.aws/amazonlinux/amazonlinux:2023', Essential: true, Command: ['/bin/sh', '-c', 'echo <token>; sleep 15'], LogConfiguration: {LogDriver: 'awslogs', Options: {awslogs-group: <CFN log group>, awslogs-region: <region>, awslogs-stream-prefix: 'app'}}}.
  Expected: containers[0].exitCode == 0 (the shell's own status after `sleep 15` returns).
  Actual: exitCode 143 (128+SIGTERM) — the container was terminated from outside.

Cause 2 (verdict: mismatch) — cloudwatch__ecs__elbv2__logs.deploy-stack-without-log-permission-no-log-group-created:
  File "check_cloudwatch__ecs__elbv2__logs_deploy_stack_without_log_permission_no_log_group_created.py", line 72, in run
  assert "RUNNING" not in observed_statuses
  AssertionError: task without logs:CreateLogGroup reached RUNNING; observed: ['PROVISIONING', 'PROVISIONING', 'RUNNING', 'RUNNING', 'STOPPED']
  Setup: one CFN stack publishes two task definitions over the same cluster — TaskDefBad with an execution role carrying only AmazonECSTaskExecutionRolePolicy (no logs:CreateLogGroup) and TaskDefGood whose role adds logs:CreateLogGroup — each with awslogs and awslogs-create-group pointed at its own non-existent group (LogGroupBadName / LogGroupGoodName).
  Expected for the bad task: DescribeTasks polled to STOPPED with RUNNING never observed, and DescribeLogGroups(logGroupNamePrefix=LogGroupBadName) empty.
  Actual: RUNNING observed twice before STOPPED, so the assertion fails before the log-group assertion is reached.
  Positive control in the same check (not yet graded because the run stops at the assertion above): the good task must reach RUNNING and LogGroupGoodName must appear.

## Steps to Reproduce
Cause 1:
1. Deploy a CFN stack with an ECS cluster, a CloudWatch log group, and a Fargate task definition whose single essential container uses the awslogs driver (awslogs-group = the stack's log group, awslogs-stream-prefix 'app') and Command ['/bin/sh','-c','echo <token>; sleep 15'].
2. ecs RunTask(launchType='FARGATE', count=1, awsvpcConfiguration with the stack's subnets/SG, assignPublicIp='ENABLED').
3. Poll ecs DescribeTasks until lastStatus == 'STOPPED' (within 180 s), then read tasks[0].containers[0].exitCode.
   Observed: 143. Expected: 0 — the container's command completed on its own.

Cause 2:
1. Deploy a CFN stack publishing TWO Fargate task definitions on one cluster, both with awslogs + awslogs-create-group true against a log group that does not exist: TaskDefBad with an execution role that has only AmazonECSTaskExecutionRolePolicy, TaskDefGood with a role that also allows logs:CreateLogGroup.
2. ecs RunTask(TaskDefBad, FARGATE, awsvpc config as above).
3. Poll ecs DescribeTasks every 5 s until lastStatus == 'STOPPED', recording every lastStatus seen.
   Observed: ['PROVISIONING','PROVISIONING','RUNNING','RUNNING','STOPPED'].
   Expected: RUNNING never appears — PROVISIONING then STOPPED (with stopCode TaskFailedToStart and a stoppedReason beginning 'ResourceInitializationError:').
4. logs DescribeLogGroups(logGroupNamePrefix=LogGroupBadName) -> must be empty.
5. Positive control: ecs RunTask(TaskDefGood) -> must reach RUNNING, and LogGroupGoodName must appear within 60 s.
