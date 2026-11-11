Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2__elbv2__opensearch.e2e-diagnose-alb-opensearch-bad-gateway-lifecycle
Issue-Key: cloudwatch__ec2__elbv2__opensearch.e2e-diagnose-alb-opensearch-bad-gateway-lifecycle|phase#8|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# cloudwatch__ec2__elbv2__opensearch: a stack's AWS::ElasticLoadBalancingV2::Listener never leaves CREATE_IN_PROGRESS, so CloudFormation times the resource out and rolls the whole stack back

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateListener

## Description
One net-new root cause (grounded in a check whose replay key is not carried by any open issue).

Cause 1 of 1: a native AWS::ElasticLoadBalancingV2::Listener in a CloudFormation stack is provisioned — a well-formed listener ARN is minted for it — but its CloudFormation resource status never advances from CREATE_IN_PROGRESS to CREATE_COMPLETE. CloudFormation's create wait for that resource expires, the resource is marked FAILED with the reason '<resource type> <physical id> was still CREATE_IN_PROGRESS when the wait for it to be created ran out', and the stack reaches ROLLBACK_COMPLETE instead of CREATE_COMPLETE. Failing check id: cloudwatch__ec2__elbv2__opensearch.e2e-diagnose-alb-opensearch-bad-gateway-lifecycle

AWS: Smithy (elasticloadbalancingv2) models CreateListener as a synchronous AWS Query operation — input {LoadBalancerArn (required), Protocol, Port (range 1-65535), DefaultActions (required), ...} -> output {Listeners}; every modeled error is a client 4xx refusal (DuplicateListener, IncompatibleProtocols, InvalidLoadBalancerAction, TargetGroupNotFound, LoadBalancerNotFound, UnsupportedProtocol, ...) and the Listener shape carries no pending/creating state member at all. There is therefore no asynchronous settling step for a listener: an AWS::ElasticLoadBalancingV2::Listener resource reaches CREATE_COMPLETE as soon as CreateListener returns 200. The template here asks for a plain HTTP:80 listener on an internal application load balancer whose single DefaultActions entry forwards to an ip-type HTTPS:443 target group in the same VPC — a configuration AWS accepts, so the only AWS-correct outcomes are CREATE_COMPLETE, or CREATE_FAILED whose reason is one of the modeled elbv2 error codes. Neither an indefinite CREATE_IN_PROGRESS nor a wait timeout is an AWS outcome.
Emulator: the listener's physical id is assigned (the failure reason quotes arn:aws:elasticloadbalancing:<region>:<account>:listener/app/<alb-name>/<alb-id>/<listener-id>), so the create itself ran, yet the resource stays CREATE_IN_PROGRESS until CloudFormation gives up. DescribeStacks reports StackStatus 'ROLLBACK_COMPLETE' and the listener's logical id ('HttpListener') carries a *_FAILED status with ResourceStatusReason whose fixed text is 'was still CREATE_IN_PROGRESS when the wait for it to be created ran out'; the leading resource type and ARN are values from the request and the emulator's state, and the stack/ALB names and id segments vary per run and are not part of the requirement.
Why: the normal way an ALB, its target groups, its listener, its listener rule and its CloudWatch alarm are created together is one CloudFormation stack. Because the stack rolls back, every resource and every Output the template produced (VpcId, DomainName, DomainEndpoint, AlbDnsName, AlbArn, DashboardsTgArn, ApiTgArn, AlarmName) is torn down and unavailable, and the client sees a rollback with no actionable AWS error code — indistinguishable from a genuine defect in its own template.
Fix: the CloudFormation resource provisioner/stabilization path for the AWS::ElasticLoadBalancingV2::Listener resource type — the step that must mark the resource CREATE_COMPLETE once elbv2 CreateListener returns. Either completion is never reported back to the stack's resource state machine, or it waits on a listener readiness condition the elbv2 state never satisfies (the modeled Listener has no such state). Any configuration the service rejects must surface as CREATE_FAILED carrying the modeled elbv2 error code, not as a wait timeout.
Traces: cloudwatch__ec2__elbv2__opensearch.e2e-diagnose-alb-opensearch-bad-gateway-lifecycle — phase 1/5 (deploy) fails before any later phase runs. NOTE for the fixer: the open issue 20261111T083559Z-cloudwatch-ec2-elbv2-opensearch-a-stack-s-aws.md describes this same cause from the SIBLING check cloudwatch__ec2__elbv2__opensearch.e2e-diagnose-alb-opensearch-stale-targets-with-alarm-config (same template); it does not carry this check's replay key, so this check is filed separately and both should close together. Also related but distinct (all concern Custom::/Lambda-backed resources, not a native listener): 20261110T220525Z-cloudformation-docdb-dynamodb-ecs-elbv2-cloudformation-a-custom-resource-never-leaves-create-in.md.

## Evidence
Expected (AWS): the AWS::ElasticLoadBalancingV2::Listener resource reaches CREATE_COMPLETE as soon as CreateListener returns 200 (Smithy: CreateListener -> CreateListenerOutput{Listeners}; no state member on Listener; all modeled errors are client 4xx), and DescribeStacks reports StackStatus 'CREATE_COMPLETE' with the stack Outputs readable. A refused configuration would instead be CREATE_FAILED with a modeled elbv2 error code (IncompatibleProtocols, InvalidLoadBalancerAction, TargetGroupNotFound, ...) as the reason.

Actual (emulator): Replay verdict: AssertionError: [phase 1/5 deploy] stack <stack> reached 'ROLLBACK_COMPLETE'; failed resources [('HttpListener', 'AWS::ElasticLoadBalancingV2::Listener arn:aws:elasticloadbalancing:us-west-2:000000000000:listener/app/<alb-name>/<alb-id>/<listener-id> was still CREATE_IN_PROGRESS when the wait for it to be created ran out')]. The stack never yields Outputs, so phases setup/solve/cleanup/destroy never execute.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite.
2. CreateStack with a template containing an AWS::EC2::VPC, private subnets, an internal AWS::ElasticLoadBalancingV2::LoadBalancer (Type application, Scheme internal), two AWS::ElasticLoadBalancingV2::TargetGroup resources (TargetType ip, Protocol HTTPS, Port 443) and one AWS::ElasticLoadBalancingV2::Listener {LoadBalancerArn: Ref Alb, Port: 80, Protocol: HTTP, DefaultActions: [{Type: forward, TargetGroupArn: Ref DashboardsTg}]}.
3. Poll DescribeStacks until StackStatus is terminal; observe 'ROLLBACK_COMPLETE'.
4. Call DescribeStackEvents and observe the listener's logical id with a *_FAILED ResourceStatus and ResourceStatusReason ending 'was still CREATE_IN_PROGRESS when the wait for it to be created ran out', while the quoted physical id is an already-minted listener ARN.
5. Replay cloudwatch__ec2__elbv2__opensearch.e2e-diagnose-alb-opensearch-bad-gateway-lifecycle to confirm.
