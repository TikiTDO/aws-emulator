Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.deploy-cfn-asg-without-elb-permission-instances-not-registered autoscaling__ec2__elbv2.deploy-cfn-stack-asg-elb-ec2-integration
Issue-Key: autoscaling__ec2__elbv2.deploy-cfn-asg-without-elb-permission-instances-not-registered|run#16|AssertionError|assert code == "AccessDenied", f"expected AccessDenied, got {code!r}"
Issue-Key: autoscaling__ec2__elbv2.deploy-cfn-stack-asg-elb-ec2-integration|_poll_until#11|Timeout|last = probe()
Issue-Commit: c3de02674d16064d9f5c8a9be8af17d3d848d40e

# autoscaling__ec2__elbv2: 2 net-new causes — AttachLoadBalancerTargetGroups answers 501 NotImplemented instead of an IAM AccessDenied, and a CloudFormation ASG+ALB+target-group stack never converges

> _Note: this issue was identified against the code base at commit `c3de02674d16064d9f5c8a9be8af17d3d848d40e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachLoadBalancerTargetGroups

## Description
Cause 1 of 2: autoscaling:AttachLoadBalancerTargetGroups is an unimplemented scaffold stub, so a call made by an identity that an identity-based policy explicitly DENIES is answered HTTP 501 NotImplemented instead of AWS's authorization refusal.

AWS: Smithy models AttachLoadBalancerTargetGroups with input AutoScalingGroupName (XmlStringMaxLen255, len 1..255) @required and TargetGroupARNs (list) @required, an empty output, and exactly three modeled errors — InstanceRefreshInProgressFault (HTTP 400, awsQuery code InstanceRefreshInProgress), ResourceContentionFault (HTTP 500) and ServiceLinkedRoleFailure (HTTP 500). NotImplemented is not one of them. Authorization is evaluated BEFORE the operation runs: a caller whose identity-based policy carries an explicit Deny on autoscaling:AttachLoadBalancerTargetGroups is refused with error code AccessDenied. Its message template, as AWS words this class, is the fixed text 'User: ' + <caller principal ARN> + ' is not authorized to perform: ' + <action> + ' on resource: ' + <resource ARN> + ' with an explicit deny in an identity-based policy' — fixed text is everything outside the four variable slots; the principal ARN (arn:aws:sts::<account>:assumed-role/<role-name>/<session-name>), the action string (autoscaling:AttachLoadBalancerTargetGroups) and the resource ARN (the group's AutoScalingGroupARN) all come from the request and the emulator's own state, never from any one recording.

Emulator: the restricted-identity call returns error code 'NotImplemented' — the check asserts `code == "AccessDenied"` and fails with 'AssertionError: expected AccessDenied, got \'NotImplemented\''. The handler is services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/attach_load_balancer_target_groups.rs, still carrying the SCAFFOLDED marker. Two defects compound here: the operation is unwritten, and no IAM authorization decision is evaluated for it at all, so neither the allowed nor the denied path is faithful. (The stub alone is filed for the plain `autoscaling` service; that issue does not carry this check's replay key and does not cover the missing AccessDenied behaviour seen here.)

Cause 2 of 2: a CloudFormation stack that declares an AWS::AutoScaling::AutoScalingGroup with a launch template alongside an AWS::ElasticLoadBalancingV2::LoadBalancer/TargetGroup/Listener never reaches an observable converged state.

AWS: recorded behavior only — the recording shows the stack reaching CREATE_COMPLETE, DescribeStacks exposing the TargetGroupArn/AutoScalingGroupName outputs, DescribeAutoScalingGroups reporting at least one instance with LifecycleState='InService', and elbv2:DescribeTargetHealth reporting that instance as a registered target that becomes healthy.

Emulator: autoscaling__ec2__elbv2.deploy-cfn-stack-asg-elb-ec2-integration ends with 'Check timed out after 60.0s' — the stack/ASG/target-group chain never reaches the first observable step, so neither the stack status, the InService instance, nor the target registration can be read back.

Why: (1) a client that relies on IAM to block a target-group attachment gets a 501 server fault instead of a 403 AccessDenied, so permission boundaries are untestable and the SDK classifies the refusal as retryable rather than as a policy decision; (2) any CloudFormation- or CDK-driven ASG-behind-ALB deployment — the single most common load-balanced pattern — never completes, so a client waiting on stack completion or on target health waits forever.

Fix: for cause 1, implement services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/attach_load_balancer_target_groups.rs (record the target-group ARNs on the group, answer HTTP 200 empty, refuse with InstanceRefreshInProgressFault where modeled) AND run the request through the shared IAM authorization layer so an explicit Deny yields AccessDenied with AWS's sentence before the handler runs. For cause 2, the CloudFormation resource provisioners for AWS::AutoScaling::AutoScalingGroup / AWS::ElasticLoadBalancingV2::* together with the Auto Scaling group reconciliation loop and the ASG->target-group registration path.

Traces: cause 1 — autoscaling__ec2__elbv2.deploy-cfn-asg-without-elb-permission-instances-not-registered; cause 2 — autoscaling__ec2__elbv2.deploy-cfn-stack-asg-elb-ec2-integration.

## Evidence
Cause 1 — Expected (AWS): restricted_autoscaling.attach_load_balancer_target_groups(AutoScalingGroupName=<g>, TargetGroupARNs=[<tg-arn>]) with an explicit Deny on autoscaling:AttachLoadBalancerTargetGroups -> Error.Code='AccessDenied', Error.Message='User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: autoscaling:AttachLoadBalancerTargetGroups on resource: <AutoScalingGroupARN> with an explicit deny in an identity-based policy'.
Actual (emulator): Error.Code='NotImplemented' — 'AssertionError: expected AccessDenied, got \'NotImplemented\'' at check line 203. The operation is also never attached when allowed, since the handler only returns the 501 stub.

Cause 2 — Expected (AWS): the CFN stack (SecurityGroup + LaunchTemplate + TargetGroup + LoadBalancer + Listener + AutoScalingGroup) reaches CREATE_COMPLETE, DescribeAutoScalingGroups reports >=1 instance at LifecycleState='InService' and DescribeTargetHealth lists it.
Actual (emulator): 'Error: Check timed out after 60.0s' — no observable step is ever reached.

## Steps to Reproduce
Cause 1:
1. Deploy the ASG + ALB + target-group CFN stack and read the TargetGroupArn and AutoScalingGroupName outputs.
2. Create an IAM role whose policy Allows autoscaling:Describe*/elasticloadbalancing:Describe*/ec2:Describe* and explicitly Denies autoscaling:AttachLoadBalancerTargetGroups; assume it.
3. Call autoscaling:AttachLoadBalancerTargetGroups(AutoScalingGroupName=<asg>, TargetGroupARNs=[<tg-arn>]) with that identity.
4. Observe error code NotImplemented instead of AccessDenied with the explicit-deny sentence.

Cause 2:
1. cloudformation:CreateStack with a template declaring AWS::EC2::SecurityGroup, AWS::EC2::LaunchTemplate, AWS::ElasticLoadBalancingV2::TargetGroup/LoadBalancer/Listener and AWS::AutoScaling::AutoScalingGroup.
2. Wait for the stack to reach a terminal status, then poll DescribeAutoScalingGroups for an InService instance and DescribeTargetHealth for its registration — neither ever arrives.
