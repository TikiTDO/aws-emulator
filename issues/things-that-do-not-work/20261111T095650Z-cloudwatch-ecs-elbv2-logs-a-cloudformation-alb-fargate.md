Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__elbv2__logs.deploy-ecs-alb-service-stack-routes-traffic
Issue-Key: cloudwatch__ecs__elbv2__logs.deploy-ecs-alb-service-stack-routes-traffic|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Commit: 19132400f00c6250ea4e57ba1f7117d500668e37

# cloudwatch__ecs__elbv2__logs: a CloudFormation ALB + Fargate ECS stack never converges to a routed, healthy target, so the check never reaches a verdict

> _Note: this issue was identified against the code base at commit `19132400f00c6250ea4e57ba1f7117d500668e37`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
Cause 1 of 1: the ALB-target-group-to-ECS-service wiring deployed by CloudFormation never reaches the healthy, traffic-serving state a client polls for.

AWS: recorded behavior only — after the stack reaches CREATE_COMPLETE, elbv2 DescribeTargetHealth reports the Fargate task's target with TargetHealth.State `healthy` and HTTP requests to the ALB's DNS name are routed to the task, all within the check's budget.

Emulator: the check is terminated with `Check timed out after 60.0s` without reaching any assertion — neither the stack's outputs-driven polls (target health) nor the data-plane request ever settles, and no terminal failure is produced either.

Why: a client that deploys the standard ALB + Fargate stack and waits for a healthy target (the normal readiness gate before sending traffic) blocks indefinitely instead of either succeeding or getting a resource failure it can act on.

Fix: the convergence path behind this stack — the ECS service/task lifecycle, its registration into the ELBv2 target group and the target-health reconciler, plus the ALB data plane that must answer on the published DNS name — reached through the AWS::ECS::Service / AWS::ElasticLoadBalancingV2::TargetGroup providers.

Traces: cloudwatch__ecs__elbv2__logs.deploy-ecs-alb-service-stack-routes-traffic.

## Evidence
Check cloudwatch__ecs__elbv2__logs.deploy-ecs-alb-service-stack-routes-traffic [mismatch]:
  Error: Check timed out after 60.0s
  The check deploys the stack, then polls elbv2.describe_target_health(TargetGroupArn=<stack output>) for healthy targets and issues HTTP requests against the ALB DNS name.
  Expected (AWS): target health reaches "healthy" and the HTTP request is routed to the running task.
  Actual (emulator): no converged state is ever observed; the check is cut off with no assertion reached.

## Steps to Reproduce
1. Deploy the check's CloudFormation stack (ALB, listener, target group, ECS cluster, Fargate service, log group).
2. Poll elbv2 DescribeTargetHealth on the stack's TargetGroupArn output for TargetHealth.State == "healthy", then GET the ALB DNS name.
3. Observe neither ever settles.
4. Re-replay cloudwatch__ecs__elbv2__logs.deploy-ecs-alb-service-stack-routes-traffic to confirm a fix.
