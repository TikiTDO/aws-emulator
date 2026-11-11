Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecs__elbv2.ecsnt-scaling-the-isolated-service-up-stops-its-task-before-any-target-registers
Issue-Key: ec2__ecs__elbv2.ecsnt-scaling-the-isolated-service-up-stops-its-task-before-any-target-registers|run#17|AssertionError|assert (svc["desiredCount"], svc["runningCount"]) == (1, 0), (
Issue-Commit: dff6e291e5e6489707888b0049fa49b367909ca3

# A service task that cannot pull its image in an isolated subnet is reported RUNNING: runningCount 1 instead of 0

> _Note: this issue was identified against the code base at commit `dff6e291e5e6489707888b0049fa49b367909ca3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServices

## Description
Cause 1 of 1: scaling an ECS service whose tasks run in subnets with no route to the registry reports the task as running instead of failing it to start.

AWS: Smithy: DescribeServices -> Service.desiredCount / Service.runningCount; DescribeTasks -> Task.stopCode is the TaskStopCode enum, whose members include TaskFailedToStart, with Task.stoppedReason carrying the CannotPullContainerError text. The recording shows that after UpdateService sets desiredCount=1 for a service placed in isolated subnets, the task stops with stopCode TaskFailedToStart (CannotPullContainerError), the service reports desiredCount/runningCount = 1/0, and the target group never reports a healthy target — while the same task definition in a public subnet reaches RUNNING.

Emulator: the service reports desiredCount/runningCount = 1/1 — 'AssertionError: desired/running after the failed start: 1/1'. The emulator counts the task as running without ever attempting (or modelling the failure of) the image pull, so no task ever carries stopCode TaskFailedToStart and the isolated-subnet negative case is indistinguishable from the healthy one.

Why: a client diagnosing "no traffic" cannot tell a task that never started from one that is serving. runningCount is the primary signal every deploy gate, alarm and runbook uses; reporting 1/1 for a task that cannot pull its image makes the emulator certify broken deployments as healthy and hides the CannotPullContainerError that tells the operator the subnet has no egress/registry route.

Fix: the ECS task lifecycle component — the placement/startup path must model the image pull against the task's network configuration, stop the task with stopCode TaskFailedToStart and a CannotPullContainerError stoppedReason when the registry is unreachable, and the service's runningCount accounting must only count tasks that actually reached RUNNING.

Traces: ec2__ecs__elbv2.ecsnt-scaling-the-isolated-service-up-stops-its-task-before-any-target-registers. The open issue 20261102T085002Z-ec2-ecs-elbv2-3-net-new-ecs-root.md covers other checks of this service and does not carry this check's replay key.

## Evidence
Expected (AWS): DescribeServices gives (desiredCount, runningCount) = (1, 0) after the failed start; DescribeTasks reports stopCode='TaskFailedToStart' and a stoppedReason naming CannotPullContainerError; the target group has no healthy target.
Actual (emulator): (desiredCount, runningCount) = (1, 1) — 'AssertionError: desired/running after the failed start: 1/1'; the task is treated as running and never stops.

## Steps to Reproduce
1. Create a VPC with isolated subnets (no IGW/NAT route) and a public subnet.
2. RegisterTaskDefinition (FARGATE/awsvpc) with a public registry image.
3. CreateService in the isolated subnets behind a target group, then UpdateService desiredCount=1.
4. Poll DescribeServices: AWS settles at runningCount 0 with the task STOPPED (stopCode TaskFailedToStart); the emulator reports runningCount 1.
5. Control: run the same definition in the public subnet — it reaches RUNNING on both.
