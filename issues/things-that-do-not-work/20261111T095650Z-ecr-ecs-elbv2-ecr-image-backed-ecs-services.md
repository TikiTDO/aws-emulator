Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__elbv2.deploy-cfn-ecs-ecr-elbv2-stack-provisions-routing ecr__ecs__elbv2.deploy-cfn-updates-service-on-image-tag-change
Issue-Key: ecr__ecs__elbv2.deploy-cfn-ecs-ecr-elbv2-stack-provisions-routing|_poll_until#13|Timeout|last = probe()
Issue-Key: ecr__ecs__elbv2.deploy-cfn-updates-service-on-image-tag-change|_poll_until#15|Timeout|last = probe()
Issue-Commit: 19132400f00c6250ea4e57ba1f7117d500668e37

# ecr__ecs__elbv2: ECR-image-backed ECS services behind an ALB never converge, so neither the initial deploy nor the image-tag update reaches a verdict

> _Note: this issue was identified against the code base at commit `19132400f00c6250ea4e57ba1f7117d500668e37`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
Cause 1 of 1: a CloudFormation stack that runs an ECR-sourced image on a Fargate service behind an ALB never reaches the healthy/registered state its clients poll for, on create or on update.

AWS: recorded behavior only — after the stack reaches CREATE_COMPLETE, the task pulls its image through the ECR pull-through cache repository the stack wires, the task registers into the ALB target group and elbv2 DescribeTargetHealth reports it `healthy`; after the image-tag parameter is changed, the service rolls to new tasks that re-register healthy on the same, unchanged target group. Both complete inside the checks' budgets.

Emulator: both checks are terminated with `Check timed out after 60.0s` without reaching an assertion. No task ever becomes a healthy registered target, and no terminal failure (e.g. a task stopped with an image-pull reason) is reported either, so the poll has nothing to observe.

Why: the normal container-deployment loop — deploy an ECR image, wait for healthy targets, then roll a new tag — cannot be driven: a waiter that AWS terminates never terminates here, and a client cannot distinguish "still converging" from "will never converge".

Fix: the ECS task lifecycle for ECR-sourced images (pull through the cached repository, task RUNNING, task stopped with a reason on failure), its ELBv2 target registration and health reconciliation, and the AWS::ECS::Service provider's update (rolling) path.

Traces: ecr__ecs__elbv2.deploy-cfn-ecs-ecr-elbv2-stack-provisions-routing, ecr__ecs__elbv2.deploy-cfn-updates-service-on-image-tag-change.

## Evidence
Two checks under ### ecr__ecs__elbv2 fail identically:
  [mismatch] ecr__ecs__elbv2.deploy-cfn-ecs-ecr-elbv2-stack-provisions-routing — Error: Check timed out after 60.0s
  [mismatch] ecr__ecs__elbv2.deploy-cfn-updates-service-on-image-tag-change — Error: Check timed out after 60.0s

Expected (AWS): the ECR-cached image runs, the Fargate task registers healthy in the target group, and an image-tag change rolls the service to new tasks that re-register healthy on the same target group.
Actual (emulator): no such state is ever reached and no terminal failure is produced; both checks are cut off with no assertion reached.

## Steps to Reproduce
1. Create an ECR pull-through cache rule for public.ecr.aws and deploy the check's stack (ECS cluster, task definition referencing the cached ECR image URI, Fargate service, ALB, listener, target group).
2. Poll elbv2 DescribeTargetHealth for a healthy target.
3. Update the stack's image-tag parameter and poll for the rolled tasks to re-register healthy.
4. Observe that neither state is ever reached.
5. Re-replay both checks in trace_ids to confirm a fix.
