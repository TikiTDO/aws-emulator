Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.deploy-elbv2-1-rule-update-tg-replace-deletion-protection
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elastic-load-balancing-v2|DescribeRules|ListenerNotFound
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# ELBv2 DescribeRules answers ListenerNotFound for the listener ARN a CREATE_COMPLETE CloudFormation stack exports, so the CFN-managed ALB update flow is unreadable

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRules

## Description
One root cause, grounded in 1 failing behavioral check whose id's leading token is 'elbv2'.

Related open issues that do NOT carry this check's replay key, and so do not cover it: 20261023T205437Z-cloudformation-reports-create-complete-for-resources-it-never.md (same family of symptom, but enumerated for ELBv2 target groups, trust stores and ECS services and keyed to 'cloudformation'-token checks), 20261023T205437Z-elbv2-ten-control-plane-operation-families-answer-501.md (CreateRule and other families answer 501 on the direct API path) and 20261024T182404Z-elbv2-listener-rule-priority-collision-update-path-never.md (the priority-collision update flow).

Cause 1 of 1: a CloudFormation stack that declares an AWS::ElasticLoadBalancingV2::Listener plus two AWS::ElasticLoadBalancingV2::ListenerRule resources settles CREATE_COMPLETE and exports a listener ARN through its Outputs, but that ARN does not exist in ELBv2's state: the very first ELBv2 read of it, DescribeRules(ListenerArn=<that ARN>), is refused with ListenerNotFound.

AWS: Smithy com.amazonaws.elasticloadbalancingv2#DescribeRules(ListenerArn, RuleArns, Marker, PageSize @range 1..400) returns Rules/NextMarker and models ListenerNotFoundException with awsQueryError {code: 'ListenerNotFound', httpResponseCode: 400} — that error means the listener does not exist. DescribeListeners models the same error. A stack only reaches CREATE_COMPLETE once every declared resource was actually created, and `{"Ref": "Listener"}` on an AWS::ElasticLoadBalancingV2::Listener resolves to the ARN of a listener that ELBv2 then serves: DescribeRules on it returns the stack's two rules, with the priorities the template declared ('10' and '50') and their FixedResponseConfig bodies. Recorded behavior only for the terminal stack status string, which the check asserts as 'CREATE_COMPLETE'.

Emulator: the stack create does settle CREATE_COMPLETE and describe_stacks returns the Outputs with resolved OutputValues (AlbArn, AlbDns, ListenerArn, TgArn, RuleFirstArn, RuleSecondArn, VpcId), but describe_rules(ListenerArn=Outputs['ListenerArn']) raises ListenerNotFoundException — 'One or more listeners not found'. The check dies there, at its first control-plane read, so nothing downstream is exercised: the lower-priority-wins rule ordering, the in-place rule update keeping the same rule ARN, the TargetGroup Name change forcing group replacement while the listener updates in place, deletion protection forcing DELETE_FAILED, and the retried delete reaching DELETE_COMPLETE.

Why: a client that deploys an ALB through CloudFormation and then drives it on the ELBv2 API — exactly what every IaC-plus-verification pipeline does — is told the deployment succeeded and handed a resource identifier the service immediately denies the existence of. There is no way to reconcile the two views: the stack cannot be trusted as a statement that the listener exists, and ListenerNotFound cannot be trusted as a statement that it does not, so the client cannot tell a deployment failure from a read bug and typically retries forever or deletes a stack it believes is broken. It also silently voids every negative-path guarantee the flow rests on (deletion protection, immutable-property replacement).

Fix: whichever side of the pair drops the listener — the CloudFormation provisioner for AWS::ElasticLoadBalancingV2::Listener / ::ListenerRule, which must create the listener and its rules in ELBv2's own state (and fail the resource, not report CREATE_COMPLETE, if it cannot) rather than minting an ARN for Ref/Outputs; and the ELBv2 listener state plus the DescribeRules/DescribeListeners lookup, which must resolve a listener created through that path and only raise ListenerNotFound (400) for an ARN that genuinely has no listener.

Traces: elbv2.deploy-elbv2-1-rule-update-tg-replace-deletion-protection (fails in _rules_by_arn -> describe_rules, immediately after the CREATE_COMPLETE assertion and the Outputs read).

## Evidence
Replay report, service elbv2:
  [error] elbv2.deploy-elbv2-1-rule-update-tg-replace-deletion-protection
  botocore.errorfactory.ListenerNotFoundException: An error occurred (ListenerNotFound) when calling the DescribeRules operation: One or more listeners not found

Sequence observed: create_stack(template with Vpc + 2 subnets + IGW + SG + Alb + Tg + Listener + RuleFirst(Priority 10, path /a, fixed-response 'v1') + RuleSecond(Priority 50, path /a, fixed-response 'v2')) -> _wait_stack returns 'CREATE_COMPLETE' (assertion PASSES) -> _outputs() yields resolved OutputValues including 'ListenerArn' (the KeyError: 'OutputValue' symptom of the older CFN issue is NOT present here) -> describe_rules(ListenerArn=Outputs['ListenerArn']) is refused.

Expected (AWS): DescribeRules on that listener ARN returns the stack's rules — Rules[RuleFirstArn].Priority == '10' and Rules[RuleSecondArn].Priority == '50' — and then the flow continues: an in-place update to Priority 5 / body 'v1b' keeps both rule ARNs and shows Priority '5' and MessageBody 'v1b'; a TargetGroup Name change replaces the group (new TgArn) while the listener ARN is unchanged and its default action forwards to the new group; enabling deletion_protection.enabled makes delete_stack end DELETE_FAILED; clearing it makes the retried delete end DELETE_COMPLETE.

Actual (emulator): HTTP 400 awsQuery Error.Code 'ListenerNotFound' on the first DescribeRules, for an ARN the CREATE_COMPLETE stack just exported. Every later assertion is unreachable.

Smithy citations (services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/smithy-model/com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10.json):
  DescribeRules input {ListenerArn, RuleArns, Marker, PageSize @range 1..400}, output {Rules, NextMarker}, errors include ListenerNotFoundException awsQueryError {code: 'ListenerNotFound', httpResponseCode: 400} and RuleNotFoundException {code: 'RuleNotFound', 400}.
  DescribeListeners errors include the same ListenerNotFoundException {code: 'ListenerNotFound', 400}.

## Steps to Reproduce
1. Boot the emulator.
2. create_stack with a self-contained template: a VPC, two public subnets in two AZs, an internet gateway and a security group; an internet-facing AWS::ElasticLoadBalancingV2::LoadBalancer; an AWS::ElasticLoadBalancingV2::TargetGroup; an AWS::ElasticLoadBalancingV2::Listener on HTTP:80 whose default action forwards to that target group; and two AWS::ElasticLoadBalancingV2::ListenerRule resources on {"Ref": "Listener"} — RuleFirst at Priority 10 (path-pattern /a, fixed-response 200 'v1') and RuleSecond at Priority 50 (path-pattern /a, fixed-response 200 'v2'). Declare Outputs AlbArn, AlbDns, ListenerArn, TgArn, RuleFirstArn, RuleSecondArn, VpcId.
3. Poll describe_stacks(StackName=<stack id>) until the status leaves *_IN_PROGRESS: it is CREATE_COMPLETE.
4. Read listener_arn = {o['OutputKey']: o['OutputValue'] for o in Stacks[0]['Outputs']}['ListenerArn'] — it is present and resolved.
5. Call boto3.client('elbv2').describe_rules(ListenerArn=listener_arn). The emulator answers HTTP 400 ListenerNotFound ('One or more listeners not found'); AWS returns the two rules with Priority '10' and '50'.
6. Cross-check with describe_listeners(ListenerArns=[listener_arn]) and describe_load_balancers(LoadBalancerArns=[Outputs['AlbArn']]) to see how much of the stack's ELBv2 estate actually exists in service state.
7. Re-replay elbv2.deploy-elbv2-1-rule-update-tg-replace-deletion-protection.
