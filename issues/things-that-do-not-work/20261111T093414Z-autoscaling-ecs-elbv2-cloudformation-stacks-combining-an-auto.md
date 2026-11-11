Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.deploy-asg-with-elb-target-group-attachment-cfn autoscaling__ecs__elbv2.deploy-ecs-service-with-alb-target-group-cfn autoscaling__ecs__elbv2.deploy-asg-missing-trust-policy-fails-instance-registration
Issue-Key: autoscaling__ecs__elbv2.deploy-asg-with-elb-target-group-attachment-cfn|_poll_until#11|Timeout|last = probe()
Issue-Key: autoscaling__ecs__elbv2.deploy-ecs-service-with-alb-target-group-cfn|_poll_until#5|Timeout|last = probe()
Issue-Key: autoscaling__ecs__elbv2.deploy-asg-missing-trust-policy-fails-instance-registration|_poll_until#13|Timeout|last = probe()
Issue-Commit: c3de02674d16064d9f5c8a9be8af17d3d848d40e

# autoscaling__ecs__elbv2: CloudFormation stacks combining an Auto Scaling group / ECS service with ELBv2 target groups never reach an observable converged state (1 cause, 3 checks)

> _Note: this issue was identified against the code base at commit `c3de02674d16064d9f5c8a9be8af17d3d848d40e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CloudFormation stack that provisions an Auto Scaling group (or an ECS service) together with AWS::ElasticLoadBalancingV2 target-group resources never reaches an observable converged state — no terminal stack status, no InService ASG instance, and no registered/healthy target — so three distinct scenarios all stall at their first poll.

AWS: recorded behavior only — the model says nothing about stack convergence timing. The recordings show, for each of these stacks: the stack reaching a terminal CREATE_COMPLETE (or CREATE_FAILED/ROLLBACK_COMPLETE for the negative permission-boundary variant), DescribeStacks exposing its Outputs with values, autoscaling:DescribeAutoScalingGroups reporting the group's instances walking Pending -> InService, and elasticloadbalancing:DescribeTargetHealth listing those instances (or the ECS task's awsvpc ip) as targets whose TargetHealth.State becomes 'healthy'. For the missing-trust-policy variant the recording shows the instance NEVER registering while the good-role twin does, which is exactly the contrast the check grades.

Emulator: all three checks end with the replay verdict 'Error: Check timed out after 60.0s', before any of those reads returns anything to assert on. autoscaling__ecs__elbv2.deploy-asg-with-elb-target-group-attachment-cfn (group 'deploy-behavior') never sees its stack-declared ASG register into the stack's target group; autoscaling__ecs__elbv2.deploy-ecs-service-with-alb-target-group-cfn (group 'deploy-behavior') never sees its ECS service's task become an ALB target; autoscaling__ecs__elbv2.deploy-asg-missing-trust-policy-fails-instance-registration (group 'permission-boundary') never reaches the point where the bad-role group's non-registration can be distinguished from the good-role group's registration.

Why: a client that deploys the standard load-balanced pattern through CloudFormation or CDK — ASG or ECS service behind an ELBv2 target group — and then waits on stack completion, on DescribeAutoScalingGroups reporting InService, or on DescribeTargetHealth reporting a healthy target, waits indefinitely. Nothing downstream of the deployment is reachable, so neither the happy path nor the permission-boundary contrast (a role without the required trust policy must leave the instance unregistered) can be relied on.

Fix: the CloudFormation resource provisioners for AWS::AutoScaling::AutoScalingGroup, AWS::ECS::Service and AWS::ElasticLoadBalancingV2::TargetGroup/LoadBalancer/Listener (stack status must reach a terminal value and Outputs must carry values), together with the Auto Scaling group reconciliation loop that must advance an instance to LifecycleState=InService and the ASG/ECS -> target-group registration path that must publish those instances as targets with a real TargetHealth state. Related open issues (20261104T042502Z-autoscaling-ecs-elbv2-6-net-new-root-causes.md, 20261107T080429Z-autoscaling-ecs-elbv2-6-net-new-causes-asg.md) do not carry these three checks' replay keys and so do not cover them.

Traces: autoscaling__ecs__elbv2.deploy-asg-with-elb-target-group-attachment-cfn, autoscaling__ecs__elbv2.deploy-ecs-service-with-alb-target-group-cfn, autoscaling__ecs__elbv2.deploy-asg-missing-trust-policy-fails-instance-registration.

## Evidence
Expected (AWS): cloudformation:DescribeStacks reports a terminal stack status with populated Outputs; autoscaling:DescribeAutoScalingGroups reports >=1 instance with LifecycleState='InService'; elbv2:DescribeTargetHealth reports those instances (or the ECS awsvpc task ip) as targets reaching TargetHealth.State='healthy'. In the permission-boundary variant the instance launched under a role lacking the required trust policy stays unregistered while the good-role twin registers.

Actual (emulator): each of the three checks ends with 'Error: Check timed out after 60.0s' — none of those reads ever yields the state being polled for, so no status, no InService instance and no target registration is ever observable.

## Steps to Reproduce
1. cloudformation:CreateStack with a template declaring an AWS::EC2::SecurityGroup, an IAM instance role, an AWS::EC2::LaunchTemplate, an AWS::ElasticLoadBalancingV2::TargetGroup (+ LoadBalancer + Listener) and an AWS::AutoScaling::AutoScalingGroup referencing that target group (second variant: an AWS::ECS::Cluster + TaskDefinition + Service with loadBalancers pointing at the target group; third variant: two groups, one whose instance role has no trust policy for ec2.amazonaws.com).
2. Poll cloudformation:DescribeStacks for a terminal status and read the stack Outputs.
3. Poll autoscaling:DescribeAutoScalingGroups for an instance at LifecycleState='InService'.
4. Poll elasticloadbalancing:DescribeTargetHealth(TargetGroupArn=<tg>) for that instance/ip as a target reaching 'healthy'.
5. Observe that steps 2-4 never produce the expected state; the scenario stalls until the suite budget expires.
