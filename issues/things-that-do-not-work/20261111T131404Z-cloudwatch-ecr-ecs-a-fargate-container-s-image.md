Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs.ecs-task-role-assumed-for-container-permissions
Issue-Key: cloudwatch__ecr__ecs.ecs-task-role-assumed-for-container-permissions|run#25|AssertionError|assert final["containers"][0].get("exitCode") == 0, f"aws cli invocation failed: {final['containers'][0]}"
Issue-Commit: ffc9519ba35ae474158cb1300960bfc0fdc8c7a4

# cloudwatch__ecr__ecs: a Fargate container's image is never executed — the task stops with a canned exitCode 127 and no in-container output, so the task role identity is unobservable

> _Note: this issue was identified against the code base at commit `ffc9519ba35ae474158cb1300960bfc0fdc8c7a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
One net-new cause.

Cause 1 of 1: the emulated Fargate container does not run the image's entrypoint with the container definition's command, and DescribeTasks reports the container STOPPED with exitCode 127. Failing check id: cloudwatch__ecr__ecs.ecs-task-role-assumed-for-container-permissions.

AWS: a container definition with image public.ecr.aws/aws-cli/aws-cli:<ver> and command ["sts","get-caller-identity"] runs the image's ENTRYPOINT (the aws CLI) with that command appended as its arguments. The process succeeds, so DescribeTasks reports the stopped container with exitCode 0 (Smithy: ecs#Container.exitCode, "The exit code returned from the container"; ecs#Task.stopCode enum includes EssentialContainerExited for a container that ran and exited), and the container's stdout is delivered to the awslogs stream <prefix>/<containerName>/<taskId>. Because the task definition carries a taskRoleArn distinct from its executionRoleArn, the credentials handed to the container are the TASK role's, so the printed identity Arn is the task role's assumed-role ARN (arn:aws:sts::<account>:assumed-role/<taskRoleName>/<session>) and never the execution role's. The fixed requirements here are: a container whose process succeeds reports exitCode 0; the container's output reaches the configured awslogs stream; in-task AWS calls are signed with the task role. The account id, role names, task id and session name are values from the request and the emulator's state, not part of the requirement.

Emulator: the task reaches lastStatus STOPPED with the container reporting exitCode 127 ('command not found'), healthStatus UNKNOWN, cpu '0' and no reason — i.e. the image's entrypoint was never applied and no image process was executed; 127 is served as a canned exit status. The check fails at the exit-code assertion ('aws cli invocation failed: {... 'exitCode': 127 ...}') and so never reaches the awslogs stream, where the task role's assumed-role identity would have had to appear and the execution role's would have had to be absent.

Why: a client cannot verify anything that depends on the container actually running: the container exit status is wrong (127 for a program that exits 0), nothing is written to the configured awslogs destination, and the task-role-vs-execution-role credential split — the whole point of taskRoleArn — is unobservable. Any workload, test or deployment gate that reads a container's exit code or its logs, or that relies on in-task AWS API calls being signed with the task role, draws the wrong conclusion.

Fix: the ECS task data plane that runs a placed task's containers (the container execution component behind RunTask/DescribeTasks, plus the awslogs delivery path and the task credential provider that must vend the taskRoleArn's credentials to the container) — it must apply the image config's ENTRYPOINT with containerDefinitions.command as arguments, report the real process exit status in Container.exitCode, and stream the container's stdout/stderr to the configured log group/stream.

Related but not covering: open issues such as 20261111T040849Z-ecs-iam-s3-a-fargate-task-never-runs-so-a-task.md and 20261111T030649Z-ecs-logs-6-net-new-causes-execution-role.md record container-never-runs symptoms under other service groups; this check is listed under cloudwatch__ecr__ecs and is not marked owned by any open issue.

Traces: cloudwatch__ecr__ecs.ecs-task-role-assumed-for-container-permissions (mismatch).

## Evidence
Check cloudwatch__ecr__ecs.ecs-task-role-assumed-for-container-permissions, body line 83:
  assert final["containers"][0].get("exitCode") == 0, f"aws cli invocation failed: {final['containers'][0]}"
  AssertionError: aws cli invocation failed: {'containerArn': 'arn:aws:ecs:...:container/...', 'taskArn': 'arn:aws:ecs:...:task/...', 'name': 'app', 'image': 'public.ecr.aws/aws-cli/aws-cli:2.36.44', 'lastStatus': 'STOPPED', 'exitCode': 127, 'networkBindings': [], 'networkInterfaces': [{...}], 'healthStatus': 'UNKNOWN', 'managedAgents': [], 'cpu': '0'}

Expected (AWS): DescribeTasks on the stopped task -> containers[0].exitCode == 0; the awslogs stream <stream-prefix>/app/<taskId> contains the caller identity of the TASK role (a substring 'assumed-role/<taskRoleName>/') and does NOT contain 'assumed-role/<execRoleName>/'.
Actual (emulator): containers[0].exitCode == 127 (the image ENTRYPOINT was not applied to the command ["sts","get-caller-identity"], so no image process ran) and no container output is produced, so the identity assertions are never reachable.

## Steps to Reproduce
1. Create a cluster and a CloudWatch Logs log group.
2. Create an execution role trusting ecs-tasks.amazonaws.com with AmazonECSTaskExecutionRolePolicy attached, and a SEPARATE task role with the same trust policy.
3. RegisterTaskDefinition (FARGATE/awsvpc) with executionRoleArn = exec role, taskRoleArn = task role, and one container 'app' on image public.ecr.aws/aws-cli/aws-cli:2.36.44, command ["sts","get-caller-identity"], logConfiguration awslogs -> the log group with a stream prefix.
4. RunTask in the default subnet/security group; poll DescribeTasks until lastStatus == STOPPED.
5. Read containers[0].exitCode: the emulator answers 127 where AWS answers 0, and GetLogEvents on <prefix>/app/<taskId> yields no caller-identity output at all.
