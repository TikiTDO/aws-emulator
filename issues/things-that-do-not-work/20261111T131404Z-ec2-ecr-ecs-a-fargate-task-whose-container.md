Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecr__ecs.ecsnrt-fargate-task-on-unpushed-tag-fails-pull-until-retagged
Issue-Key: ec2__ecr__ecs.ecsnrt-fargate-task-on-unpushed-tag-fails-pull-until-retagged|run#19|AssertionError|assert g["stopCode"] == "TaskFailedToStart", f"the control stopped with {g.get('stopCode')!r}"
Issue-Commit: ffc9519ba35ae474158cb1300960bfc0fdc8c7a4

# ec2__ecr__ecs: a Fargate task whose container cannot be started reports stopCode EssentialContainerExited instead of TaskFailedToStart

> _Note: this issue was identified against the code base at commit `ffc9519ba35ae474158cb1300960bfc0fdc8c7a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
One net-new cause.

Cause 1 of 1: a Fargate task whose container never starts is classified as a container that ran and exited. Failing check id: ec2__ecr__ecs.ecsnrt-fargate-task-on-unpushed-tag-fails-pull-until-retagged.

AWS: the task definition's container points at an ECR image whose image config declares Entrypoint ["/nonexistent"], i.e. an executable that does not exist in the layer. ECS cannot create/start the container, so the task goes to lastStatus STOPPED with task.stopCode = "TaskFailedToStart" (Smithy: ecs#Task.stopCode -> enum ecs#TaskStopCode with values TaskFailedToStart | EssentialContainerExited | UserInitiated | ServiceSchedulerInitiated | SpotInterruption | TerminationNotice | InfrastructureHealth; the member doc points at the Stopped tasks error codes guide, where a container that cannot be started is a start failure, surfaced as CannotStartContainerError in stoppedReason / Container.reason). EssentialContainerExited is reserved for the other case: the essential container DID start, ran, and then exited — in which case Container.exitCode carries the exit status of the process (Smithy: ecs#Container.exitCode, "The exit code returned from the container"). The two classes are distinguished by whether the container ever started, not by the task's eventual outcome. Variable parts of the record (task/container ARNs, the digest, timestamps) come from the request and the emulator's state; the fixed part that is a requirement is the stopCode token itself.

Emulator: DescribeTasks reports the STOPPED task with stopCode "EssentialContainerExited" for a container that could not be started at all, so every task that fails before its process exists is reported as a process that ran and exited. The check asserts on the control task (the one on the pushed tag, with the same unrunnable entrypoint) and fails at that first assertion: 'the control stopped with EssentialContainerExited'.

Why: a client diagnosing a stopped task keys off stopCode — TaskFailedToStart means 'the platform could not start your container' (bad entrypoint, unpullable image, unwritable log destination, role/network setup) and sends the operator to the task's stoppedReason, while EssentialContainerExited means 'your process ran and exited' and sends them to the application exit code and logs. Collapsing start failures into EssentialContainerExited makes the emulator report a successful container start that never happened, so deployment-failure diagnosis, circuit breakers and retry logic all draw the wrong conclusion. It also hides the complementary signals this scenario checks: a container that never started has no imageDigest and the task has no startedAt.

Fix: the ECS task data-plane state machine that drives a task to STOPPED (the RunTask/task-lifecycle component and the state it writes for DescribeTasks) must distinguish 'container never started' from 'essential container exited', set stopCode = TaskFailedToStart with the corresponding stoppedReason/Container.reason for the former, and leave Container.exitCode unset and Task.startedAt unset when no container process ever ran.

Related but not covering: open issue 20261111T030957Z-ecr-ecs-elbv2-container-image-pull-failures-are.md records the same misclassification for the ecr__ecs__elbv2 service group and only for image-pull failures; this check is in a different service group and the start failure here is an unrunnable entrypoint on an image that pulls fine.

Traces: ec2__ecr__ecs.ecsnrt-fargate-task-on-unpushed-tag-fails-pull-until-retagged (mismatch).

## Evidence
Check ec2__ecr__ecs.ecsnrt-fargate-task-on-unpushed-tag-fails-pull-until-retagged, body line 64:
  assert g["stopCode"] == "TaskFailedToStart", f"the control stopped with {g.get('stopCode')!r}"
  AssertionError: the control stopped with 'EssentialContainerExited'

Expected (AWS): DescribeTasks on the STOPPED Fargate task whose image config Entrypoint is ["/nonexistent"] -> task.stopCode == "TaskFailedToStart", container reason/stoppedReason naming the start failure (CannotStartContainerError), container.imageDigest == the pushed manifest digest for the pushed tag and absent for the never-pushed tag, task.startedAt absent for the task that never started.
Actual (emulator): task.stopCode == "EssentialContainerExited".

## Steps to Reproduce
1. CreateRepository, then push a single-layer image whose image config declares config.Entrypoint = ["/nonexistent"] via InitiateLayerUpload/UploadLayerPart/CompleteLayerUpload + PutImage under tag dev_build_service_main.
2. Create an execution role trusting ecs-tasks.amazonaws.com with AmazonECSTaskExecutionRolePolicy attached, a VPC + public subnet, and a cluster.
3. RegisterTaskDefinition (FARGATE, awsvpc, cpu 256, memory 512, executionRoleArn) with one essential container on <repositoryUri>:dev_build_service_main.
4. RunTask with launchType FARGATE and assignPublicIp ENABLED; poll DescribeTasks until lastStatus == STOPPED.
5. Read task.stopCode: the emulator answers "EssentialContainerExited" where AWS answers "TaskFailedToStart" (the container's entrypoint does not exist, so no container process ever ran).
