Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__elbv2.deploy-cfn-ecs-ecr-elbv2-stack-provisions-routing ecr__ecs__elbv2.deploy-cfn-updates-service-on-image-tag-change
Issue-Key: ecr__ecs__elbv2.deploy-cfn-ecs-ecr-elbv2-stack-provisions-routing|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: ecr__ecs__elbv2.deploy-cfn-updates-service-on-image-tag-change|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Commit: d9e20b4dbdf06191d17fc8b16aa8f32f57ea4689

# ecr__ecs__elbv2: an ECR-image-backed Fargate service behind an ALB never converges — no task ever becomes a registered healthy target and no terminal failure is reported, on create or on image-tag update

> _Note: this issue was identified against the code base at commit `d9e20b4dbdf06191d17fc8b16aa8f32f57ea4689`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
NOTE: the open issue 20261111T095650Z-ecr-ecs-elbv2-ecr-image-backed-ecs-services.md describes the same symptom, but the replay suite does NOT report these two checks as OWNED by it (its recorded Issue-Key replay keys — `_poll_until#13` / `_poll_until#15` — no longer bind to the current checks), so the checks are still unowned and are re-filed here with the keys the suite reports today. That issue should be treated as the same work item.

Cause 1 of 1: a CloudFormation stack that runs an ECR-sourced image on a Fargate service behind an Application Load Balancer never reaches the state its clients poll for, neither on the initial deploy nor after the image tag is rolled.

AWS: recorded behavior only. Once the stack reaches CREATE_COMPLETE, the Fargate task pulls the image through the ECR pull-through cache repository the stack wires (`<acct>.dkr.ecr.<region>.amazonaws.com/<prefix>/nginx/nginx:<tag>` backed by public.ecr.aws), the task's awsvpc ENI gets an IP, ECS registers that IP into the ALB target group named in the service's loadBalancers block, and elbv2 DescribeTargetHealth returns a TargetHealthDescription whose TargetHealth.State becomes `healthy`. When the image-tag parameter is changed, the AWS::ECS::Service provider performs a rolling update: new tasks start, register on the SAME unchanged target group and reach `healthy` again, and the stack reaches UPDATE_COMPLETE. Either way the state settles — and when it cannot settle, AWS reports a TERMINAL outcome instead (the task is stopped with a stoppedReason such as a CannotPullContainerError, and the stack ends in CREATE_FAILED/ROLLBACK_COMPLETE), so a poll always terminates.

Emulator: both checks are cut off with `Error: Check timed out after 60.0s` without ever reaching an assertion. The poll loop around DescribeTargetHealth never observes a target in state `healthy` (the healthy count stays 0), and the emulator also never produces the terminal alternative — no stopped task with a pull reason, no failed/rolled-back stack — so there is nothing for the waiter to observe in either direction. The emulator serves every control-plane call, so the divergence is in the asynchronous convergence, not in a refusal.

Why: a client cannot drive the standard container-deployment loop — deploy an ECR image, wait for healthy ALB targets, then roll a new tag and wait again. Every AWS waiter built on DescribeTargetHealth / DescribeServices (and the CloudFormation stack waiter for the update) hangs forever instead of terminating, and a client cannot distinguish "still converging" from "will never converge", because neither the success state nor a failure reason is ever reported.

Fix: the ECS task lifecycle for ECR-sourced images (pull through the pull-through-cache repository, task to RUNNING with an ENI address, or task STOPPED carrying a stoppedReason on pull failure), the ELBv2 target-registration and health-reconciliation path that turns a running task into a TargetHealthDescription with State `healthy`, and the AWS::ECS::Service CloudFormation provider's update (rolling-deployment) path so an image-tag change rolls tasks and the stack settles.

Traces: ecr__ecs__elbv2.deploy-cfn-ecs-ecr-elbv2-stack-provisions-routing, ecr__ecs__elbv2.deploy-cfn-updates-service-on-image-tag-change.

## Evidence
Two checks under `### ecr__ecs__elbv2` fail identically in the replay report:
  [mismatch] ecr__ecs__elbv2.deploy-cfn-ecs-ecr-elbv2-stack-provisions-routing — Error: Check timed out after 60.0s
  [mismatch] ecr__ecs__elbv2.deploy-cfn-updates-service-on-image-tag-change — Error: Check timed out after 60.0s

Both checks deploy the same template (ECR pull-through cache rule for public.ecr.aws, ECS cluster, awsvpc FARGATE task definition whose container image is the cached ECR URI, ALB + listener + ip-type target group, ECS service with DesiredCount 1 and a loadBalancers binding) and then poll:
  health = elbv2.describe_target_health(TargetGroupArn=tg_arn)['TargetHealthDescriptions']
  sum(1 for t in health if t['TargetHealth']['State'] == 'healthy') >= 1

Expected (AWS): at least one TargetHealthDescription with TargetHealth.State == 'healthy' after the deploy, and again after the ImageTag parameter is updated and the service rolls; or, on failure, a terminal task stop reason plus a failed/rolled-back stack.
Actual (emulator): the healthy count never leaves 0, no stopped-task reason and no stack failure are produced, and both checks are terminated by the harness budget with no assertion reached.

Neither failure depends on a per-run value (host, port, generated stack/cluster/service name, timestamp or request id); the divergence is that no terminal state of any kind is reached.

## Steps to Reproduce
1. Create an ECR pull-through cache rule with UpstreamRegistryUrl `public.ecr.aws` and some EcrRepositoryPrefix `<prefix>`.
2. CreateStack with the check's template: ECS cluster; IAM execution role trusting ecs-tasks.amazonaws.com with AmazonECSTaskExecutionRolePolicy plus ecr:CreateRepository and ecr:BatchImportUpstreamImage; an awsvpc FARGATE task definition (Cpu 256 / Memory 512) whose container `app` image is `${AWS::AccountId}.dkr.ecr.${AWS::Region}.amazonaws.com/<prefix>/nginx/nginx:latest` with containerPort 80; an internet-facing ALB over two subnets; an HTTP:80 target group with TargetType `ip` and HealthCheckPath `/`; an HTTP:80 listener forwarding to it; and an ECS::Service with DesiredCount 1, LaunchType FARGATE and a LoadBalancers entry binding container `app` port 80 to the target group. Wait for CREATE_COMPLETE.
3. Call elbv2 DescribeTargetHealth on the stack's TargetGroupArn output and look for a TargetHealthDescription with TargetHealth.State == 'healthy'. Observe that none ever appears, and that DescribeTasks reports no stopped task with a pull reason either.
4. UpdateStack changing the ImageTag parameter (e.g. latest -> 1-alpine) and poll DescribeTargetHealth again for a healthy target on the same, unchanged target group. Observe the same non-convergence.
5. Re-replay both check ids listed in trace_ids to confirm a fix.
