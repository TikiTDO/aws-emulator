Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__logs.svcreplace-stopped-standalone-task-is-never-relaunched-while-a-service-managed-one-is-replaced
Issue-Key: ecs__logs.svcreplace-stopped-standalone-task-is-never-relaunched-while-a-service-managed-one-is-replaced|_drop#4|Timeout|time.sleep(2)
Issue-Commit: 7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae

# ecs__logs: a StopTask'd service task is never replaced and a standalone task's container never logs, so the replace-vs-never-relaunch distinction is unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopTask

## Description
Cause 1 of 1: the ECS service steady-state loop and the container data plane never progress, so the difference between a service-managed task (replaced after StopTask) and an identical standalone RunTask task (never relaunched) is never observable.
AWS: recorded behavior only. For a desiredCount=1 service, StopTask on its task makes the service's scheduler launch a replacement task that reaches lastStatus RUNNING on the same task definition, while an identical task launched by RunTask outside any service is NOT relaunched after StopTask. Task attribution is visible on the records: a service task reports group "service:<serviceName>", a standalone task does not report a group beginning with "service:". Both containers really run: each writes its boot line and, on StopTask, its SIGTERM farewell line into its OWN awslogs stream and exits with the handler's code 42, which DescribeTasks reports as the container's exitCode. The log text and the service name are values taken from the request and state; the requirement is that the replacement, the non-replacement, the group attribution, the two log lines and the exit code are all observable.
Emulator: the scenario never reaches a verdict — the check is killed by the harness at 60s. The tasks do not progress through RUNNING/STOPPED, no container output reaches the awslogs streams, and no replacement task appears, so a client cannot distinguish a service-managed task from a standalone one.
Why: a client relying on ECS's steady-state guarantee (a stopped service task is replaced, a stopped standalone task is not) sees neither behaviour, and nothing it can observe — group, log events, exitCode — ever materialises.
Fix: the service steady-state reconciliation that replaces a stopped service task, the standalone-task path that must NOT relaunch, the task lifecycle progression through RUNNING to STOPPED with the container's exitCode and SIGTERM handling, and the awslogs delivery of container STDOUT (the ecs service crate's service/task state machine and its log-driver delivery).
Traces: ecs__logs.svcreplace-stopped-standalone-task-is-never-relaunched-while-a-service-managed-one-is-replaced

Note for the fixer: the open issue 20261104T062603Z-ecs-logs-the-awslogs-driver-never-delivers-awsvpc.md describes overlapping symptoms but the replay report did not mark this check as owned by it, so this check's replay key is filed here; if it is the same root cause, close them together and re-replay the check id below.

## Evidence
Expected: DescribeTasks shows the service's task with group == "service:<serviceName>" and the standalone task with no "service:" group; after StopTask on each, the service's task is replaced by a NEW task that reaches lastStatus RUNNING on the same taskDefinitionArn while the standalone task is never relaunched; each container's own awslogs stream contains its boot line and its SIGTERM farewell line, and each stopped container reports exitCode 42.
Actual: "Check timed out after 60.0s" — the poll loops for RUNNING/replacement and for log events never converge; no container output and no replacement task are produced.

## Steps to Reproduce
1. Replay the behavioural suite and re-run ecs__logs.svcreplace-stopped-standalone-task-is-never-relaunched-while-a-service-managed-one-is-replaced.
2. Directly: create a CloudWatch Logs log group and an execution role; RegisterTaskDefinition (FARGATE, awsvpc, logConfiguration awslogs -> that log group, container command printing a boot line and installing a SIGTERM handler that prints a farewell line and exits 42).
3. CreateCluster; CreateService with desiredCount=1 on that task definition and awsvpcConfiguration; wait for one RUNNING service task and read its group.
4. RunTask the SAME task definition standalone on the same cluster; wait for RUNNING and read its group.
5. StopTask both; then poll ListTasks/DescribeTasks for a replacement of the service task (expected) and for no relaunch of the standalone task (expected), and GetLogEvents on each container's stream for the boot and farewell lines plus the exitCode 42.
