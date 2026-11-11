Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target elbv2.xff-header-processing-mode-changes-what-the-target-receives
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target|_poll_until#31|Timeout|time.sleep(interval_s)
Issue-Key: elbv2.xff-header-processing-mode-changes-what-the-target-receives|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Commit: deeb1944d1d0db8b9764b546c3d5c87bdcf95957

# elbv2: 2 net-new causes — routing.http.desync_mitigation_mode=strictest never refuses an ambiguously framed request, and routing.http.xff_header_processing_mode never changes what the target receives

> _Note: this issue was identified against the code base at commit `deeb1944d1d0db8b9764b546c3d5c87bdcf95957`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyLoadBalancerAttributes

## Description
Two net-new root causes: two load-balancer HTTP attributes that ModifyLoadBalancerAttributes accepts and DescribeLoadBalancerAttributes echoes are inert on the data path.

=== Cause 1 of 2: routing.http.desync_mitigation_mode = strictest does not block an ambiguously framed request before the target ===
AWS: with routing.http.desync_mitigation_mode set to "strictest" (Smithy: ModifyLoadBalancerAttributes / LoadBalancerAttribute Key+Value, the attribute keys are documented prose on the operation), an ALB refuses any request whose framing is ambiguous — e.g. one carrying both Content-Length and Transfer-Encoding, or a malformed Transfer-Encoding — and answers it itself with a load-balancer-generated HTTP 400 without forwarding anything to the target. The target must receive no request at all.
Emulator: recorded behavior only — the check never reaches a verdict and is killed ("Check timed out after 60.0s"); the ambiguous request is neither answered 400 by the load balancer nor observably dropped, so the attribute has no effect on the data path.
Why: desync mitigation is a security control. A client that sets "strictest" and validates it against the emulator is told the control is in place when request smuggling is not actually blocked.
Fix: the ALB data-plane HTTP request parser must apply the load balancer's stored routing.http.desync_mitigation_mode when framing headers are ambiguous, generating the 400 at the load balancer. Open issues 20261105T151413Z-elbv2-routing-http-desync-mitigation-mode-is-inert.md and 20261105T093535Z-elbv2-4-net-new-data-plane-causes-desync.md describe the same attribute being inert but do not carry this check key; this check was replayed and is still failing.
Traces: elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target.

=== Cause 2 of 2: routing.http.xff_header_processing_mode does not change the X-Forwarded-For header the target receives ===
AWS: routing.http.xff_header_processing_mode selects how the ALB treats an incoming X-Forwarded-For header: "append" (default) adds the client address to the existing list, "preserve" forwards the client-supplied header unchanged, and "remove" strips it before forwarding. The header the target sees differs per mode, and in "append" mode the ALB's appended value is the layer-3 client address, not anything the client claimed.
Emulator: recorded behavior only — the check never reaches a verdict and is killed ("Check timed out after 60.0s"); no forwarded request is observed at the target for any mode, so the attribute cannot take effect.
Why: applications derive the client IP for authorization, rate limiting and audit from X-Forwarded-For. A client cannot validate which mode its ALB is in, and emulator-based tests of client-IP handling exercise nothing.
Fix: the ALB data plane's request-forwarding path must read the load balancer's stored routing.http.xff_header_processing_mode and append/preserve/remove the X-Forwarded-For header accordingly before the request reaches the target.
Traces: elbv2.xff-header-processing-mode-changes-what-the-target-receives.

## Evidence
Both checks: "Error: Check timed out after 60.0s" with no assertion reached.
Cause 1 expected: with routing.http.desync_mitigation_mode=strictest, a request with conflicting Content-Length/Transfer-Encoding framing is answered HTTP 400 by the load balancer and the target records no request.
Cause 2 expected: with routing.http.xff_header_processing_mode in turn append/preserve/remove, the target observes the client-supplied X-Forwarded-For respectively extended with the connecting address, passed through unchanged, and absent.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite so the emulator is serving.
2. Create an ALB with an HTTP listener forwarding to a target group with a reachable target.
3. Cause 1: ModifyLoadBalancerAttributes with routing.http.desync_mitigation_mode=strictest, then send a raw request carrying both Content-Length and Transfer-Encoding to the load balancer and observe the response and whether the target saw the request.
4. Cause 2: ModifyLoadBalancerAttributes with routing.http.xff_header_processing_mode set to append, then preserve, then remove, sending a request with a client-supplied X-Forwarded-For each time and inspecting the header the target received.
