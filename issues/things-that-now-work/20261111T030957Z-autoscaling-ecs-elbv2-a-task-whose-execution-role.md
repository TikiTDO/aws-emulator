Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.ecs-run-task-fails-without-ecs-tasks-trust-policy
Issue-Key: autoscaling__ecs__elbv2.ecs-run-task-fails-without-ecs-tasks-trust-policy|run#15|AssertionError|assert "RUNNING" not in statuses_seen, (
Issue-Commit: deeb1944d1d0db8b9764b546c3d5c87bdcf95957

# autoscaling__ecs__elbv2: a task whose execution role does not trust ecs-tasks.amazonaws.com still reaches RUNNING instead of stopping with TaskFailedToStart

> _Note: this issue was identified against the code base at commit `deeb1944d1d0db8b9764b546c3d5c87bdcf95957`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 1: ECS does not validate that the task's execution role trusts ecs-tasks.amazonaws.com before starting the container.

AWS: RunTask accepts the task, but the task never reaches lastStatus RUNNING when the execution role's trust policy does not allow sts:AssumeRole for the ecs-tasks.amazonaws.com service principal — ECS cannot assume the role, so the task goes PROVISIONING -> PENDING -> STOPPED with Task.stopCode = "TaskFailedToStart" (Smithy: ecs Task.stopCode targets TaskStopCode, whose values are TaskFailedToStart, EssentialContainerExited, UserInitiated, ServiceSchedulerInitiated, SpotInterruption, TerminationNotice, InfrastructureHealth) and Task.stoppedReason naming the role-assumption failure ("ECS was unable to assume the configured role '<role-arn>'. Please verify that the ECS service principal is allowed to assume this role." — fixed text with the execution role ARN from the task definition). No container runs, so nothing is written to the configured awslogs log group.
Emulator: the same task definition's task is observed in RUNNING (statuses seen: PROVISIONING, STOPPED, RUNNING) and the container executes; the role's trust policy is never consulted. The task's eventual stopCode is therefore the container-exit code, not TaskFailedToStart.
Why: an untrusted or mis-trusted execution role is one of the most common real ECS misconfigurations. Against the emulator such a task appears to work, so the failure mode is only discovered in production, and every negative test for execution-role trust passes vacuously. It also means stopCode is not usable to distinguish "never started" from "started and exited".
Fix: RunTask's task-start path in the ecs service — resolve and check the task definition's executionRoleArn trust policy against the ecs-tasks.amazonaws.com principal before transitioning the task past PENDING, and set stopCode/stoppedReason on the task state when the check fails.
Traces: autoscaling__ecs__elbv2.ecs-run-task-fails-without-ecs-tasks-trust-policy.

## Evidence
Expected: the task never reports lastStatus RUNNING, Task.stopCode == "TaskFailedToStart", and FilterLogEvents on the configured log group returns no events.
Actual: AssertionError: container should never start when the execution role can't be assumed, but observed statuses={'PROVISIONING', 'STOPPED', 'RUNNING'} (check line 47). The negative role in the recording trusted only ec2.amazonaws.com.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite so the emulator is serving.
2. Create an ECS cluster and a CloudWatch Logs group.
3. Create an IAM role whose trust policy allows sts:AssumeRole only for ec2.amazonaws.com, and register a Fargate task definition using it as executionRoleArn with an awslogs log configuration pointing at that group.
4. RunTask with an awsvpcConfiguration and poll DescribeTasks: the task is observed RUNNING instead of going STOPPED with stopCode TaskFailedToStart.
