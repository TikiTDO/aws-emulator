Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudfront__ec2__elbv2.deploy-loadbalancer-invalid-scheme-rejected
Issue-Key: cloudformation__cloudfront__ec2__elbv2.deploy-loadbalancer-invalid-scheme-rejected|run#8|AssertionError|assert got == _REJECTED, f"handler reason {got!r}, expected {_REJECTED!r}"
Issue-Commit: 19132400f00c6250ea4e57ba1f7117d500668e37

# cloudformation__cloudfront__ec2__elbv2: a CloudFormation resource-handler failure reason drops the relayed SDK error envelope (Service/Status Code/Request ID) from the ELBv2 message

> _Note: this issue was identified against the code base at commit `19132400f00c6250ea4e57ba1f7117d500668e37`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
Cause 1 of 1: the ResourceStatusReason CloudFormation publishes for a failed resource carries only the service's message text, without the SDK exception envelope AWS appends.

AWS: Smithy (elasticloadbalancingv2) CreateLoadBalancer.Scheme targets LoadBalancerSchemeEnum (internet-facing | internal) and the operation models InvalidSchemeException (awsQueryError code `InvalidScheme`, HTTP 400); a value outside the enum is refused 400 with the validation message `1 validation error detected: Value '<scheme>' at 'scheme' failed to satisfy constraint: Member must satisfy enum value set: [internet-facing, internal]`. CloudFormation's AWS::ElasticLoadBalancingV2::LoadBalancer handler relays that exception's full rendering, so the stack event's ResourceStatusReason is `Resource handler returned message: "<service message> (Service: ElasticLoadBalancingV2, Status Code: 400, Request ID: <request-id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)`. Fixed text: the quoted service message, then ` (Service: `, `, Status Code: `, `, Request ID: `, `) (SDK Attempt Count: `, `)`; the variable parts are the offending value from the request, the service id and status of the refusing call, and the per-run request id / attempt count (per-run values are not themselves requirements — their presence and shape are).

Emulator: the reason stops at the service message — the recorded handler reason is `("1 validation error detected: Value 'sim2real-bogus-scheme' at 'scheme' failed to satisfy constraint: Member must satisfy enum value set: [internet-facing, internal]", 'InvalidRequest')`, with no `(Service: ..., Status Code: ..., Request ID: ...) (SDK Attempt Count: ...)` segment. The message text and HandlerErrorCode are right; only the relayed envelope is missing.

Why: clients and IaC tooling parse ResourceStatusReason structurally to learn which service refused a resource and with what HTTP status (that is how a 400 validation refusal is told from a throttle or a 5xx). With the envelope absent, those parsers find no match and either misreport the failure or treat the reason as unstructured prose.

Fix: the CloudFormation resource-handler failure-reason serializer (the component that renders a downstream service error into ResourceStatusReason), which must append the refusing call's service id, HTTP status, request id and SDK attempt count inside the quoted handler message.

Traces: cloudformation__cloudfront__ec2__elbv2.deploy-loadbalancer-invalid-scheme-rejected.

## Evidence
Check cloudformation__cloudfront__ec2__elbv2.deploy-loadbalancer-invalid-scheme-rejected [mismatch]:
  AssertionError: handler reason ("1 validation error detected: Value 'sim2real-bogus-scheme' at 'scheme' failed to satisfy constraint: Member must satisfy enum value set: [internet-facing, internal]", 'InvalidRequest'),
  expected ("1 validation error detected: Value 'sim2real-bogus-scheme' at 'scheme' failed to satisfy constraint: Member must satisfy enum value set: [internet-facing, internal] (Service: ElasticLoadBalancingV2, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)", 'InvalidRequest')

Expected (AWS): quoted handler message = service message + " (Service: ElasticLoadBalancingV2, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)".
Actual (emulator): quoted handler message = service message only.

## Steps to Reproduce
1. Create a stack with an AWS::ElasticLoadBalancingV2::LoadBalancer whose Scheme is a value outside the enum (e.g. "sim2real-bogus-scheme").
2. Wait for the stack to fail and read the LoadBalancer resource's CREATE_FAILED ResourceStatusReason via DescribeStackEvents.
3. Observe the quoted handler message ends at "...[internet-facing, internal]" with no " (Service: ElasticLoadBalancingV2, Status Code: 400, Request ID: ...) (SDK Attempt Count: ...)" suffix.
4. Re-replay cloudformation__cloudfront__ec2__elbv2.deploy-loadbalancer-invalid-scheme-rejected to confirm a fix.
