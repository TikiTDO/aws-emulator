Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.deploy-cfn-alb-nonexistent-subnet-rejected elbv2.deploy-elbv2-1-rule-update-tg-replace-deletion-protection elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected elbv2.zero-weight-target-group-is-never-selected
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.deploy-cfn-alb-nonexistent-subnet-rejected|run#13|AssertionError|assert any(bad_subnet in r and "does not exist" in r for r in reasons), (
Issue-Key: elbv2.deploy-elbv2-1-rule-update-tg-replace-deletion-protection|run#27|AssertionError|assert status == "UPDATE_COMPLETE", f"update #1 ended {status!r}: {_failed_events(cfn, stack_id)!r}"
Issue-Key: elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected|run#25|AssertionError|assert reasons, f"no UPDATE_FAILED event for RuleA; failures were {_failed_events(cfn, stack_id)!r}"
Issue-Key: elbv2.zero-weight-target-group-is-never-selected|_http#24|Timeout|except urllib.error.HTTPError as exc:
Issue-Commit: 19132400f00c6250ea4e57ba1f7117d500668e37

# elbv2: 3 net-new causes — the nonexistent-subnet refusal is the bare EC2 error code, an in-place AWS::EC2::VPCGatewayAttachment update answers NotImplemented and rolls every ELBv2 stack update back, and a weighted-forward data-plane check never converges

> _Note: this issue was identified against the code base at commit `19132400f00c6250ea4e57ba1f7117d500668e37`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
Cause 1 of 3: CreateLoadBalancer's nonexistent-subnet refusal carries no message — only the bare EC2 error code.

AWS: Smithy (elasticloadbalancingv2) CreateLoadBalancer takes Subnets and models SubnetNotFoundException / InvalidSubnetException (awsQueryError codes `SubnetNotFound` / `InvalidSubnet`, HTTP 400); when a supplied subnet id does not exist, ELBv2 relays EC2's refusal with code `InvalidSubnetID.NotFound` and the sentence `The subnet ID '<subnet-id>' does not exist` — fixed text `The subnet ID '` / `' does not exist`, with the offending subnet id taken verbatim from the request. Through CloudFormation that text appears inside the LoadBalancer resource's CREATE_FAILED ResourceStatusReason.

Emulator: the handler reason is `Resource handler returned message: "InvalidSubnetID.NotFound" (RequestToken: <token>, HandlerErrorCode: NotFound)` — the error *code* is used as the whole message, so no message text and no subnet id are reported anywhere in the event stream.

Why: a client (or an operator reading stack events) cannot tell WHICH subnet is wrong, and anything matching on the AWS sentence sees no match. This is the same defect class as the already-open elbv2 issue 20261108T150952Z-elbv2-setsubnets-allocationidnotfound-message-is-the-bare-ec2.md (SetSubnets / AllocationIdNotFound), but for CreateLoadBalancer's subnet resolution, which that issue's replay key does not cover.

Fix: CreateLoadBalancer's subnet resolution/validation and the error serializer that must render the relayed EC2 message, not just its code.

Cause 2 of 3: CloudFormation refuses to update an existing AWS::EC2::VPCGatewayAttachment in place with `NotImplemented`, so every stack update in these ELBv2 scenarios rolls back before the ELBv2 change under test is attempted.

AWS: recorded behavior only — re-submitting a template whose AWS::EC2::VPCGatewayAttachment properties are unchanged (or changed) is handled by the resource provider: the attachment is updated (or no-op'd) and the stack update proceeds to UPDATE_COMPLETE, so the listener-rule/target-group change being tested is actually applied, and a genuinely invalid change (a duplicate rule priority) is the thing that fails, with RuleA reporting UPDATE_FAILED.
Emulator: the update ends UPDATE_ROLLBACK_COMPLETE with the failed event `('IgwAttach', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::EC2::VPCGatewayAttachment in place; the resource was left as it is.')`. The ELBv2 resources are never updated, and the negative check sees no UPDATE_FAILED for the rule it expects to fail.
Why: no stack that owns its own VPC and internet gateway can ever be updated — the update is refused by an unrelated resource, so both successful ELBv2 updates (rule/target-group replacement, deletion protection) and ELBv2's own update refusals (priority collision) are unreachable.
Fix: the AWS::EC2::VPCGatewayAttachment resource provider's update path (an unchanged attachment must be a no-op, a changed one must update or replace), in CloudFormation's EC2 resource providers.

Cause 3 of 3: the weighted-forward data-plane behaviour never converges — a zero-weight target group check is terminated at the harness budget.

AWS: recorded behavior only — with a forward action over two target groups where one has Weight 0, repeated requests through the ALB are all answered by the non-zero group; the requests complete and the distribution is observable inside the check's budget.
Emulator: `Check timed out after 60.0s` with no assertion reached — the ALB never serves the requests the check issues, so the weighting can never be observed.
Why: a client cannot verify (or rely on) weighted routing; a zero-weight group being excluded is the mechanism behind blue/green cutovers, and here the data plane produces nothing at all.
Fix: the ALB data plane's forward-action target selection (weighted target-group choice) and the listener's request serving. Related open elbv2 data-plane issue that does not carry this check's replay key: 20261110T220525Z-elbv2-5-net-new-data-plane-causes-dnsname.md.

Traces: Cause 1 = elbv2.deploy-cfn-alb-nonexistent-subnet-rejected; Cause 2 = elbv2.deploy-elbv2-1-rule-update-tg-replace-deletion-protection and elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected; Cause 3 = elbv2.zero-weight-target-group-is-never-selected.

## Evidence
Cause 1 — [mismatch] elbv2.deploy-cfn-alb-nonexistent-subnet-rejected:
  AssertionError: expected a stack event citing 'subnet-00000000000000000' as nonexistent, got reasons: [..., 'Resource handler returned message: "InvalidSubnetID.NotFound" (RequestToken: e35de058-..., HandlerErrorCode: NotFound)', ...]
  Expected (AWS): a reason containing the subnet id and "does not exist" — i.e. the relayed message "The subnet ID 'subnet-00000000000000000' does not exist".
  Actual (emulator): the bare code "InvalidSubnetID.NotFound" as the whole message.

Cause 2 — two checks:
  [mismatch] elbv2.deploy-elbv2-1-rule-update-tg-replace-deletion-protection
    AssertionError: update #1 ended 'UPDATE_ROLLBACK_COMPLETE': [('IgwAttach', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::EC2::VPCGatewayAttachment in place; the resource was left as it is.')]
  [mismatch] elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected
    AssertionError: no UPDATE_FAILED event for RuleA; failures were [('IgwAttach', 'UPDATE_FAILED', 'NotImplemented: ... AWS::EC2::VPCGatewayAttachment in place ...')]
  Expected (AWS): UPDATE_COMPLETE for the valid update; for the invalid one, UPDATE_FAILED on RuleA carrying ELBv2's priority-in-use refusal.
  Actual (emulator): UPDATE_ROLLBACK_COMPLETE caused by the VPCGatewayAttachment NotImplemented refusal.

Cause 3 — [mismatch] elbv2.zero-weight-target-group-is-never-selected:
  Error: Check timed out after 60.0s
  Expected (AWS): every request through the ALB is answered by the non-zero-weight target group.
  Actual (emulator): the check never gets a served request; cut off with no assertion reached.

## Steps to Reproduce
Cause 1:
1. Create a stack with AWS::ElasticLoadBalancingV2::LoadBalancer (Type=application, Scheme=internal) whose Subnets parameter is ['subnet-00000000000000000'].
2. Read the LoadBalancer CREATE_FAILED ResourceStatusReason: it is the bare code "InvalidSubnetID.NotFound"; AWS relays "The subnet ID 'subnet-00000000000000000' does not exist".

Cause 2:
1. Deploy a stack that owns its VPC, subnets, AWS::EC2::VPCGatewayAttachment (IgwAttach), ALB, listener and rules.
2. Submit an update of that same stack (e.g. changing a listener rule's priority/target group or DeletionProtection).
3. Observe UPDATE_FAILED on IgwAttach with the NotImplemented in-place-update message and UPDATE_ROLLBACK_COMPLETE for the stack.

Cause 3:
1. Create an ALB listener with a forward action over two target groups, one with Weight 0.
2. Issue repeated HTTP requests to the ALB DNS name and record which group answered.
3. Observe no request is served.

Re-replay the four checks in trace_ids to confirm a fix.
