Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__elbv2.ecs-task-fails-when-ecr-repo-policy-denies-pull ecr__ecs__elbv2.ecs-task-fails-with-nonexistent-ecr-image-tag ecr__ecs__elbv2.ecs-task-with-foreign-ecr-registry-image-fails-to-pull
Issue-Key: ecr__ecs__elbv2.ecs-task-fails-when-ecr-repo-policy-denies-pull|run#18|AssertionError|assert denied_task.get("stopCode") == "TaskFailedToStart", (
Issue-Key: ecr__ecs__elbv2.ecs-task-fails-with-nonexistent-ecr-image-tag|run#12|AssertionError|assert bad_task.get("stopCode") == "TaskFailedToStart", (
Issue-Key: ecr__ecs__elbv2.ecs-task-with-foreign-ecr-registry-image-fails-to-pull|run#13|AssertionError|assert bad_task.get("stopCode") == "TaskFailedToStart", (
Issue-Commit: deeb1944d1d0db8b9764b546c3d5c87bdcf95957

# ecr__ecs__elbv2: container image pull failures are not modeled — a task that cannot pull its image stops with EssentialContainerExited instead of TaskFailedToStart / CannotPullContainerError

> _Note: this issue was identified against the code base at commit `deeb1944d1d0db8b9764b546c3d5c87bdcf95957`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 1: the emulator never fails a task at the image-pull step, so every unpullable image is reported as a container that started and exited.

AWS: image pull happens before the container runs. When the image cannot be pulled — an ECR repository policy that denies ecr:BatchGetImage / ecr:GetDownloadUrlForLayer to the task's execution role, an image tag that does not exist in the repository, or an image whose registry hostname names another account's ECR registry the caller cannot read — the task never starts the container. DescribeTasks reports lastStatus STOPPED with Task.stopCode = "TaskFailedToStart" (Smithy: ecs Task.stopCode targets TaskStopCode = {TaskFailedToStart, EssentialContainerExited, UserInitiated, ServiceSchedulerInitiated, SpotInterruption, TerminationNotice, InfrastructureHealth}) and a failure text containing "CannotPullContainerError" (on Task.stoppedReason and/or Container.reason; Task.stoppedReason is a free-form String). The fixed part of that text is the "CannotPullContainerError:" class name and its phase wording; the variable part is the image reference from the task definition and the registry's own error, taken from the request and state.
Emulator: in all three cases the task reports stopCode 'EssentialContainerExited' and the failure text carries no CannotPullContainerError — the pull is treated as having succeeded and the container as having run and exited. Neither the repository policy, nor the existence of the tag, nor the registry account in the image reference affects the outcome.
Why: stopCode is the field clients and schedulers branch on to tell "never started" (retry/alarm on configuration) from "ran and exited" (application fault). Collapsing the two makes deploy-failure handling, service circuit breakers and operator runbooks wrong, and makes every negative test for ECR permissions, image tags and cross-account registries pass vacuously.
Fix: the task-start path in the ecs service — add an image-resolution/pull step that consults the ECR repository policy and the execution role's permissions, verifies the tag exists in the named repository, and refuses a registry hostname belonging to another account; on failure set Task.stopCode=TaskFailedToStart and a CannotPullContainerError-prefixed reason without ever transitioning the container to RUNNING. Note 20261109T141609Z-ecr-ecs-elbv2-a-container-with-a-healthcheck.md and 20261110T143849Z-ecr-ecs-elbv2-an-ecs-service-created-with.md cover different ECS/ELBv2 symptoms and do not carry these check keys.
Traces: ecr__ecs__elbv2.ecs-task-fails-when-ecr-repo-policy-denies-pull, ecr__ecs__elbv2.ecs-task-fails-with-nonexistent-ecr-image-tag, ecr__ecs__elbv2.ecs-task-with-foreign-ecr-registry-image-fails-to-pull.

## Evidence
All three checks: AssertionError: expected stopCode TaskFailedToStart, got 'EssentialContainerExited'.
- ecs-task-fails-when-ecr-repo-policy-denies-pull (line 64): after SetRepositoryPolicy with an explicit Deny on ecr:BatchGetImage/ecr:GetDownloadUrlForLayer for the execution role, the task is expected to stop TaskFailedToStart with CannotPullContainerError; the positive control (before the policy) must show no pull error.
- ecs-task-fails-with-nonexistent-ecr-image-tag (line 43): image '<repoUri>:does-not-exist-tag' in a pullable repository.
- ecs-task-with-foreign-ecr-registry-image-fails-to-pull (line 46): image '<other-account>.dkr.ecr.<region>.amazonaws.com/<repo>:v1'; the check also asserts "CannotPullContainerError" appears in the failure text.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite so the emulator is serving.
2. CreateRepository, push a minimal image tagged v1, create an execution role and an ECS cluster.
3. Case A: SetRepositoryPolicy denying ecr:BatchGetImage and ecr:GetDownloadUrlForLayer to the execution role, then RunTask a task definition using <repoUri>:v1 and read DescribeTasks stopCode/stoppedReason.
4. Case B: RunTask a task definition using <repoUri>:does-not-exist-tag.
5. Case C: RunTask a task definition using 111122223333.dkr.ecr.<region>.amazonaws.com/<repo>:v1.
6. In all three cases stopCode is 'EssentialContainerExited' and no CannotPullContainerError appears.
