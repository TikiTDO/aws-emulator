Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack
Issue-Key: cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack|run#21|AssertionError|assert m, f"SecondRule's failure reason carries no resource-handler message: {reason!r}"
Issue-Commit: 19132400f00c6250ea4e57ba1f7117d500668e37

# cloudformation__elbv2: 2 net-new causes — the ListenerRule priority-collision failure reason omits the relayed SDK envelope and classifies the clash as ResourceConflict instead of AlreadyExists

> _Note: this issue was identified against the code base at commit `19132400f00c6250ea4e57ba1f7117d500668e37`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRule

## Description
Cause 1 of 2: the ResourceStatusReason for a failed AWS::ElasticLoadBalancingV2::ListenerRule carries only ELBv2's message text, without the SDK exception envelope AWS appends.

AWS: Smithy (elasticloadbalancingv2) CreateRule models PriorityInUseException (awsQueryError code `PriorityInUse`, HTTP 400) for a Priority already used on the listener, with message `Priority '<priority>' is currently in use` (fixed text `Priority '` / `' is currently in use`; the variable part is the colliding priority from the request). CloudFormation's ListenerRule handler relays that exception in full, so the event reason reads `Resource handler returned message: "Priority '<priority>' is currently in use (Service: ElasticLoadBalancingV2, Status Code: 400, Request ID: <request-id>)" (RequestToken: <token>, HandlerErrorCode: AlreadyExists)` — the message is followed, inside the quotes, by ` (Service: ElasticLoadBalancingV2, Status Code: 400, Request ID: <id>)`.

Emulator: the recorded reason is `Resource handler returned message: "Priority '10' is currently in use" (RequestToken: <token>, HandlerErrorCode: ResourceConflict)` — the quoted message has no `(Service: ..., Status Code: ...)` segment at all, so a client parsing the reason for the refusing service and its HTTP status finds nothing.

Cause 2 of 2: the handler error code for the same failure is wrong.

AWS: a priority clash on a ListenerRule is classified by the resource handler as HandlerErrorCode `AlreadyExists` (the ELBv2 PriorityInUse 400 maps to the already-exists class).
Emulator: the same event reports HandlerErrorCode `ResourceConflict`.
Why: HandlerErrorCode is the machine-readable classification CloudFormation consumers branch on (retry, import, or report a duplicate). A different token makes an already-exists conflict look like a transient resource conflict, so clients retry a deployment that can never succeed.
Fix: the AWS::ElasticLoadBalancingV2::ListenerRule provider's error classification (mapping ELBv2 PriorityInUse to the AlreadyExists handler class).

Fix (cause 1): the CloudFormation resource-handler failure-reason serializer, which must append the refusing call's service id, HTTP status and request id inside the quoted handler message. Both causes are observed in the same stack event; the same envelope gap also shows up under the cloudformation__cloudfront__ec2__elbv2 heading (filed separately, as issues carry one service).

Traces: cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack (both causes).

## Evidence
Check cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack [mismatch]:
  AssertionError: SecondRule's failure reason carries no resource-handler message:
    'Resource handler returned message: "Priority \'10\' is currently in use" (RequestToken: 88cf041a-..., HandlerErrorCode: ResourceConflict)'
  The check parses: r'Resource handler returned message: "(?P<msg>.*?) \(Service: (?P<svc>[A-Za-z0-9]+), Status Code: (?P<status>\d+)' and then requires (msg, svc, status) == ("Priority '10' is currently in use", "ElasticLoadBalancingV2", "400") and HandlerErrorCode == "AlreadyExists".

Expected (AWS): reason = Resource handler returned message: "Priority '<priority>' is currently in use (Service: ElasticLoadBalancingV2, Status Code: 400, Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: AlreadyExists)
Actual (emulator): no (Service:, Status Code:) envelope, and HandlerErrorCode: ResourceConflict.

## Steps to Reproduce
1. Deploy a stack with an ALB, a listener and two AWS::ElasticLoadBalancingV2::ListenerRule resources that declare the same Priority (e.g. 10).
2. Wait for the stack to fail and roll back; read the second rule's UPDATE/CREATE_FAILED ResourceStatusReason via DescribeStackEvents.
3. Observe the quoted message lacks " (Service: ElasticLoadBalancingV2, Status Code: 400, Request ID: ...)" and HandlerErrorCode is ResourceConflict instead of AlreadyExists.
4. Re-replay cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack to confirm a fix.
