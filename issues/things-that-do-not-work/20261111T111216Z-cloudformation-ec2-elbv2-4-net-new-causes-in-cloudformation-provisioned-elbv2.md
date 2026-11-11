Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__elbv2.cfn-elbv2-internal-load-balancer-in-place-update-retained-replacement-then-not-found cloudformation__ec2__elbv2.cfn-elbv2-listener-fixed-response-in-place-update-retained-replacement-then-not-found cloudformation__ec2__elbv2.cfn-elbv2-listener-rule-in-place-update-retained-replacement-then-not-found cloudformation__ec2__elbv2.cfn-elbv2-target-group-in-place-update-retained-replacement-then-not-found
Issue-Key: cloudformation__ec2__elbv2.cfn-elbv2-internal-load-balancer-in-place-update-retained-replacement-then-not-found|_poll#23|TimeoutError|raise TimeoutError(f"run budget ended while waiting for {what}; last {value!r}")
Issue-Key: cloudformation__ec2__elbv2.cfn-elbv2-listener-fixed-response-in-place-update-retained-replacement-then-not-found|_error#21|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: cloudformation__ec2__elbv2.cfn-elbv2-listener-rule-in-place-update-retained-replacement-then-not-found|_expect#37|AssertionError|raise AssertionError(
Issue-Key: cloudformation__ec2__elbv2.cfn-elbv2-target-group-in-place-update-retained-replacement-then-not-found|_assert_replaced_and_retained#36|AssertionError|assert (
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# 4 net-new causes in CloudFormation-provisioned ELBv2: internal LB ENIs never settle, a gone listener still reads back, ListenerRule in-place update refused, and replacement is not reported as an update

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Four independent root causes behind the ELBv2 CloudFormation checks.

Cause 1 of 4: the network interfaces of an internal load balancer never reach the state AWS reports.
AWS: an internal (scheme 'internal') AWS::ElasticLoadBalancingV2::LoadBalancer gets one ENI per subnet, each with the ELB description 'ELB app/<name>/<id>', status 'in-use' while the LB is active, the LB's security groups, and NO public IP association. A client polling DescribeNetworkInterfaces converges on that within the LB's provisioning time.
Emulator: the ENIs never converge; the poll exhausts its budget with 'run budget ended while waiting for ENIs of <lb-name>; last [(subnet-..., ('ELB app/<name>/<id>', 'available', True, ('sg-...',))), ...]' — the interfaces stay 'available' rather than reaching the state the check requires.
Why: a client that waits for the LB's ENIs (to attach rules, to check the private addresses it will serve on, or just to know provisioning finished) waits forever.
Fix: the ELBv2 load-balancer provisioner's ENI creation/state transitions in EC2 state.
Failing check: cfn-elbv2-internal-load-balancer-in-place-update-retained-replacement-then-not-found.

Cause 2 of 4: a call that must be refused succeeds. AWS: DescribeListeners addressed to a listener ARN that no longer exists is refused with ListenerNotFound (HTTP 400); the message is ELBv2's fixed text for an absent listener and names the ARN supplied.
Emulator: the call returns HTTP 200 with the full listener body (Port 8080, Protocol HTTP, the fixed-response DefaultActions and 'listener revision one') where a refusal was required.
Why: a client cannot tell a deleted/replaced listener from a live one, so cleanup verification and idempotent re-create logic both mis-fire.
Fix: the ELBv2 DescribeListeners handler and the listener state's delete path.
Failing check: cfn-elbv2-listener-fixed-response-in-place-update-retained-replacement-then-not-found.

Cause 3 of 4: in-place update of AWS::ElasticLoadBalancingV2::ListenerRule is refused. AWS: changing a rule's conditions/actions is applied in place through ModifyRule; the rule ARN is unchanged, the resource goes UPDATE_IN_PROGRESS -> UPDATE_COMPLETE and the stack reaches UPDATE_COMPLETE.
Emulator: the resource is UPDATE_FAILED with reason 'NotImplemented: this emulator does not change the properties of an existing AWS::ElasticLoadBalancingV2::ListenerRule in place; the resource was left as it is.' and the stack ends 'UPDATE_ROLLBACK_COMPLETE' with StatusReason 'The following resource(s) failed to update: [Rule]. ' (fixed text as quoted; the variable part is the resource type and the logical id).
Why: routing rules managed by CloudFormation cannot be edited at all.
Fix: the AWS::ElasticLoadBalancingV2::ListenerRule handler's update path.
Failing check: cfn-elbv2-listener-rule-in-place-update-retained-replacement-then-not-found.

Cause 4 of 4: a replacing update of AWS::ElasticLoadBalancingV2::TargetGroup is not announced as an update and the retained old group is deleted. AWS: a replacement is reported on the logical resource as UPDATE_IN_PROGRESS (old physical id) -> CREATE_IN_PROGRESS 'Resource creation Initiated' (new ARN) -> CREATE_COMPLETE -> UPDATE_COMPLETE; with UpdateReplacePolicy Retain the old target group is not deleted.
Emulator: only CREATE_IN_PROGRESS/CREATE_COMPLETE for the new ARN then DELETE_IN_PROGRESS/DELETE_COMPLETE for the old ARN — no UPDATE_* events, and the Retain policy is ignored.
Why: stack events do not show that a replacement happened, and a target group the author asked to retain is destroyed.
Fix: the CloudFormation update engine's replacement event emission and UpdateReplacePolicy Retain handling.
Failing check: cfn-elbv2-target-group-in-place-update-retained-replacement-then-not-found.

## Evidence
Cause 1: TimeoutError: run budget ended while waiting for ENIs of sim2real-lb-a-7f8ac87b5b2c; last [('subnet-...98', ('ELB app/sim2real-lb-a-.../ab022846748b3716', 'available', True, ('sg-...86',))), ('subnet-...9a', (...))]
Cause 2: AssertionError: expected the call to be refused, but it succeeded: {'Listeners': [{'ListenerArn': 'arn:aws:elasticloadbalancing:...:listener/app/...', 'Port': 8080, 'Protocol': 'HTTP', 'DefaultActions': [{'Type': 'fixed-response', ... 'MessageBody': 'listener revision one', 'StatusCode': '503'}]}], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
Cause 3: ('Rule', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::ElasticLoadBalancingV2::ListenerRule in place; the resource was left as it is.') with the stack at 'UPDATE_ROLLBACK_COMPLETE' ('The following resource(s) failed to update: [Rule]. ')
Cause 4: AssertionError: TargetGroup must announce its replacement, its events were [('CREATE_IN_PROGRESS', None, ''), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', '<new tg arn>'), ('CREATE_COMPLETE', None, '<new tg arn>'), ('DELETE_IN_PROGRESS', None, None), ('DELETE_COMPLETE', None, '<old tg arn>')]

## Steps to Reproduce
Cause 1: CreateStack an internal ELBv2 load balancer across two subnets; poll ec2 DescribeNetworkInterfaces filtered to the LB description.
Cause 2: CreateStack an ELBv2 listener with a fixed-response default action, delete/replace it, then call DescribeListeners with the old ListenerArn.
Cause 3: CreateStack an AWS::ElasticLoadBalancingV2::ListenerRule, then UpdateStack changing its conditions/actions.
Cause 4: CreateStack an AWS::ElasticLoadBalancingV2::TargetGroup with UpdateReplacePolicy Retain, UpdateStack with a replacement-forcing change, then read DescribeStackEvents.
