Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecs__elbv2.cfn-alb-target-group-deregistration-delay-configured-positive cloudformation__ecs__elbv2.cfn-ecs-service-awsvpc-ip-target-type-positive cloudformation__ecs__elbv2.cli-cloudformation-create-stack-then-ecs-service-list-tasks
Issue-Key: cloudformation__ecs__elbv2.cfn-alb-target-group-deregistration-delay-configured-positive|probe#13|IndexError|return ecs.describe_services(cluster=cluster, services=[service])["services"][0]
Issue-Key: cloudformation__ecs__elbv2.cfn-ecs-service-awsvpc-ip-target-type-positive|probe#12|IndexError|return ecs.describe_services(cluster=cluster, services=[service])["services"][0]
Issue-Key: cloudformation__ecs__elbv2.cli-cloudformation-create-stack-then-ecs-service-list-tasks|_wait_stack_status#19|AssertionError|raise AssertionError(f"stack {stack_name} entered unexpected status {last}")
Issue-Commit: 19132400f00c6250ea4e57ba1f7117d500668e37

# cloudformation__ecs__elbv2: 2 net-new causes — an AWS::ECS::Service the stack reports created is unknown to DescribeServices, and an ECS+Fargate+ALB stack ends ROLLBACK_COMPLETE

> _Note: this issue was identified against the code base at commit `19132400f00c6250ea4e57ba1f7117d500668e37`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServices

## Description
Cause 1 of 2: a service created by an AWS::ECS::Service resource does not exist in ECS, so DescribeServices answers with an empty services list.

AWS: Smithy (ecs) DescribeServices takes cluster + services (names or ARNs) and returns `services` (the resolved services) and `failures`. A service that exists is always returned under `services` with its deployment, loadBalancers and desired/running counts; only an unknown identifier is reported, under `failures`, with reason `MISSING`. After CloudFormation reports the AWS::ECS::Service resource CREATE_COMPLETE, DescribeServices on that cluster/service must return exactly one entry under `services`.

Emulator: `ecs.describe_services(cluster=cluster, services=[service])["services"]` is empty for the stack's service, so the client's indexing raises IndexError in both checks. The CloudFormation provider reports the resource created while ECS holds no such service (or does not resolve it by the name/ARN the stack published).

Why: every post-deploy read of a CFN-provisioned ECS service breaks — deployment waiters, health and target-registration checks, scaling and rollback automation all see a service that CloudFormation says exists and ECS says does not. Clients get an empty list where AWS guarantees one service or an explicit MISSING failure.

Fix: the AWS::ECS::Service resource provider (it must create the service in ECS state under the identity it publishes as the physical resource id) and/or ecs DescribeServices' name/ARN resolution against that state.

Related open issue, which does NOT carry these checks' replay keys and is filed under a different service heading: 20261109T223435Z-cloudformation-ec2-ecs-an-ecs-service-a-stack.md.

Cause 2 of 2: a stack of an ECS cluster, a Fargate service that DependsOn the listener, and an ALB target group ends ROLLBACK_COMPLETE instead of CREATE_COMPLETE.

AWS: recorded behavior only — the stack reaches CREATE_COMPLETE, `aws ecs list-tasks` then lists exactly one task, the target group holds one healthy target and an HTTP request through the ALB returns 200.
Emulator: the stack settles ROLLBACK_COMPLETE (`stack <name> entered unexpected status ROLLBACK_COMPLETE`), so at least one resource in that ECS/ELBv2 graph fails to create and the whole deployment is rolled back.
Why: the ordinary ECS-behind-ALB deployment shape cannot be deployed at all; nothing downstream of the stack can be exercised, and a client sees a creation failure where AWS succeeds.
Fix: whichever resource provider in that graph fails — the AWS::ECS::Cluster/Service and AWS::ElasticLoadBalancingV2::TargetGroup/Listener providers and their DependsOn ordering; the failing resource's CREATE_FAILED event reason should also be surfaced.

Traces: Cause 1 = cloudformation__ecs__elbv2.cfn-alb-target-group-deregistration-delay-configured-positive and cloudformation__ecs__elbv2.cfn-ecs-service-awsvpc-ip-target-type-positive; Cause 2 = cloudformation__ecs__elbv2.cli-cloudformation-create-stack-then-ecs-service-list-tasks.

## Evidence
Cause 1 — two checks fail identically:
  [error] cloudformation__ecs__elbv2.cfn-alb-target-group-deregistration-delay-configured-positive
  [error] cloudformation__ecs__elbv2.cfn-ecs-service-awsvpc-ip-target-type-positive
    return ecs.describe_services(cluster=cluster, services=[service])["services"][0]
    IndexError: list index out of range
  Expected (AWS): services == [ { serviceName/serviceArn, deployments, loadBalancers, ... } ] for a service CloudFormation reported CREATE_COMPLETE (an unknown name would instead appear under failures with reason "MISSING").
  Actual (emulator): services == [].

Cause 2 — [mismatch] cloudformation__ecs__elbv2.cli-cloudformation-create-stack-then-ecs-service-list-tasks:
  AssertionError: stack sim2real-cfnstack-<id> entered unexpected status ROLLBACK_COMPLETE
  Expected (AWS): CREATE_COMPLETE, then one task listed, one healthy target, HTTP 200 through the ALB.
  Actual (emulator): ROLLBACK_COMPLETE.

## Steps to Reproduce
Cause 1:
1. Deploy a stack with AWS::ECS::Cluster, AWS::ECS::TaskDefinition, AWS::ElasticLoadBalancingV2::TargetGroup (+ listener) and AWS::ECS::Service.
2. Wait for CREATE_COMPLETE, then call ecs DescribeServices(cluster=<cluster>, services=[<service name or ARN from the stack>]).
3. Observe services == [] (AWS returns the one service).

Cause 2:
1. `aws cloudformation create-stack` with an ECS cluster, a Fargate service that DependsOn the ALB listener, and a target group.
2. Poll DescribeStacks: the stack ends ROLLBACK_COMPLETE instead of CREATE_COMPLETE; inspect DescribeStackEvents for the CREATE_FAILED resource and reason.

Re-replay the three checks in trace_ids to confirm a fix.
