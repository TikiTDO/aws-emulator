Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.drop-invalid-header-fields-strips-the-header-before-the-target
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.drop-invalid-header-fields-strips-the-header-before-the-target|_http#27|Timeout|except urllib.error.HTTPError as exc:
Issue-Commit: bbe224ee8535eddb09d8153d70c72e25d751373e

# drop_invalid_header_fields ALB flow never completes: no step of the attribute round-trip or the Lambda-target data plane answers inside the replay budget

> _Note: this issue was identified against the code base at commit `bbe224ee8535eddb09d8153d70c72e25d751373e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyLoadBalancerAttributes

## Description
Cause 1 of 1: the `routing.http.drop_invalid_header_fields.enabled` end-to-end flow on an Application Load Balancer produces no observable behavior at all — the check is cut off by the harness budget before any step of it resolves. Failing check id: elbv2.drop-invalid-header-fields-strips-the-header-before-the-target.

AWS: (a) ModifyLoadBalancerAttributes accepts `routing.http.drop_invalid_header_fields.enabled` only with the value `true` or `false`; any other value is refused with HTTP 400 and error code `ValidationError`, message `The value of '<attribute key>' must be 'true' or 'false', but was '<submitted value>'`. The fixed text of that message is "The value of '" … "' must be 'true' or 'false', but was '" … "'"; the two variable parts are the attribute key and the value, both taken verbatim from the request. The refused value is NOT stored: DescribeLoadBalancerAttributes keeps reporting the previous value for that key. Smithy: ModifyLoadBalancerAttributes input `Attributes` (LoadBalancerAttributes, length 0..20) and `LoadBalancerArn` are both required; its modeled errors are InvalidConfigurationRequestException (awsQueryError InvalidConfigurationRequest, HTTP 400) and LoadBalancerNotFoundException (awsQueryError LoadBalancerNotFound, HTTP 400) — the out-of-domain attribute value is answered with the service-common `ValidationError` instead, which is what the emulator's own handler already classifies it as. (b) With the attribute set to `true`, the ALB removes request headers whose names are not valid HTTP tokens (e.g. a header name containing underscores) BEFORE forwarding to the target, still forwards the request and still answers the target's HTTP 200, and delivers valid headers in the same request unchanged. With the attribute `false` (the default) both the invalid-named header and the valid one reach the target. Reference: AWS ALB documentation, load-balancer attributes (the reference the check cites). (c) A target group of TargetType `lambda` with a registered Lambda function is invoked per request, and the ALB passes the client's request headers to the function in the invocation event's `headers`/`multiValueHeaders`; health checks are off by default for a Lambda target group, so its target health stays `unused` rather than `healthy`.

Emulator: the replay produces exactly one result for this check — `Check timed out after 60.0s` — and no step-level response at all. Nothing in the scenario (create internet-facing ALB in the shared VPC, refuse the non-boolean attribute value, create the echoing Lambda, register it into a `lambda` target group, create the HTTP:80 listener, then request through the published DNSName with attribute true and then false) resolves within the replay budget. Consequently neither the strip-on-true nor the deliver-on-false behavior, nor the attribute's read-back, is observable. Hypothesis: the ALB data plane never answers a request forwarded to a `lambda` target group, so the check's bounded data-plane poll spins until it is cut off; the attribute validation itself is implemented (see Traces), so the refusal step is unlikely to be the stall.

Why: a client that enables `routing.http.drop_invalid_header_fields.enabled` to have the ALB sanitize header names gets neither the sanitization nor a served request — it hangs instead of receiving the target's 200, and it has no way to tell whether the attribute took effect. A client that relies on a Lambda target behind an ALB receives no response at all, where AWS returns the function's statusCode/body.

Fix: the elbv2 ALB data-plane request path — forwarding/invoking a `lambda` target group and answering the client with the function's response — and the header-sanitization step on that path that is gated by `routing.http.drop_invalid_header_fields.enabled`, plus the attribute's read-back through DescribeLoadBalancerAttributes. The attribute's domain validation already lives in the ModifyLoadBalancerAttributes handler and is not the component to change. Open issue 20261110T220525Z-elbv2-5-net-new-data-plane-causes-dnsname.md describes other elbv2 data-plane failures but does not carry this check's replay key, so it does not cover it.

Traces: elbv2.drop-invalid-header-fields-strips-the-header-before-the-target. Attribute-domain classification: services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/modify_load_balancer_attributes.rs:213-223.

## Evidence
Expected (AWS):
1. modify_load_balancer_attributes(LoadBalancerArn=<alb>, Attributes=[{Key: 'routing.http.drop_invalid_header_fields.enabled', Value: <non-boolean>}]) -> HTTP 400, Code `ValidationError`, Message "The value of 'routing.http.drop_invalid_header_fields.enabled' must be 'true' or 'false', but was '<non-boolean>'"; describe_load_balancer_attributes still reports the prior value for that key.
2. With the attribute 'true': GET http://<alb DNSName>/ carrying both an invalid-named header (underscores) and a valid header -> HTTP 200 from the Lambda target; the echoed request headers contain the valid header with its value and do NOT contain the invalid-named one.
3. With the attribute 'false': the same request -> HTTP 200 and the echoed headers contain BOTH headers with their values.

Actual (emulator):
- Replay verdict: [mismatch] elbv2.drop-invalid-header-fields-strips-the-header-before-the-target
- Error detail, in full: `Check timed out after 60.0s`
- No step of the scenario reported a status, an error code or a response body; the check's own bounded polls (load-balancer attribute read-back, and the data-plane echo through the published DNSName) never satisfied their predicate before the replay cut the check off, so none of expectations 1-3 was ever reached.

## Steps to Reproduce
1. Boot the emulator and replay check `elbv2.drop-invalid-header-fields-strips-the-header-before-the-target` (suite simparity-behavioral-aws-v1, service heading `elbv2`).
2. Equivalently, by hand against the emulator:
   a. ec2 describe_vpcs / describe_subnets to pick a VPC and two AZ subnets; create a security group allowing HTTP:80 ingress.
   b. elbv2 create_load_balancer(Scheme='internet-facing', Subnets=[two subnets], SecurityGroups=[sg]) and wait for State=active.
   c. elbv2 modify_load_balancer_attributes(LoadBalancerArn=<alb>, Attributes=[{Key:'routing.http.drop_invalid_header_fields.enabled', Value:'yes-please'}]) — expect ValidationError with the message above; then describe_load_balancer_attributes and confirm the value was not stored.
   d. iam create_role for Lambda; lambda create_function with a handler that echoes event['headers'] as JSON; wait for State=Active; lambda add_permission(Principal='elasticloadbalancing.amazonaws.com', SourceArn=<tg arn>).
   e. elbv2 create_target_group(TargetType='lambda'); register_targets(Targets=[{Id: <function arn>}]); create_listener(Port=80, Protocol='HTTP', DefaultActions=[forward to that target group]).
   f. GET http://<DNSName>/ with headers {'x_sim2real_bad':'dropped','X-Sim2Real-Good':'kept'} and read the echoed headers.
   g. Set the attribute to 'true', repeat (f): expect HTTP 200 with only the valid header echoed. Set it to 'false', repeat (f): expect HTTP 200 with both headers echoed.
3. Observed: the flow never reaches a served response; the replay terminates the check at its 60 s budget with `Check timed out after 60.0s`.
