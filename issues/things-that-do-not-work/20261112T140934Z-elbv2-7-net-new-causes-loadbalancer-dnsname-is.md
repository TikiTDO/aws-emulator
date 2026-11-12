Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.e2e-blue-green-weighted-target-group-shift elbv2.e2e-http-to-https-redirect-traffic-flow elbv2.deregistration-delay-serves-in-flight-requests-while-draining elbv2.elbv2shape-deep-elbv2-3-slow-start-ramps-late-target-but-not-one-call-batch elbv2.target-response-exceeding-idle-timeout-yields-504 elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-clientcert-chain elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-forged-clientcert-header-stripped elbv2.listener-ssl-policy-bounds-the-negotiated-tls-version-and-cipher elbv2.routing-http2-attribute-decides-the-alpn-protocol-the-alb-selects elbv2.forward-weighted-and-sticky-target-groups-data-plane elbv2.instance-target-serves-http-and-unhealthy-stops-routing elbv2.zero-weight-target-group-is-never-selected elbv2.xff-header-processing-mode-changes-what-the-target-receives elbv2.alb-generated-responses-excluded-from-requestcount-and-target-codes elbv2.deploy-elbv2-1-rule-update-tg-replace-deletion-protection elbv2.redirect-action-placeholder-substitution elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.e2e-blue-green-weighted-target-group-shift|_poll_http_get#23|EndpointRefused|_url, _route = ctx.endpoint_route(f"http://{host}:{port}")
Issue-Key: elbv2.e2e-http-to-https-redirect-traffic-flow|_poll_http_get#21|EndpointRefused|_url, _route = ctx.endpoint_route(f"http://{host}:{port}")
Issue-Key: elbv2.deregistration-delay-serves-in-flight-requests-while-draining|_poll_until#27|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: elbv2.elbv2shape-deep-elbv2-3-slow-start-ramps-late-target-but-not-one-call-batch|_poll_until#22|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: elbv2.target-response-exceeding-idle-timeout-yields-504|_poll_until#24|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-clientcert-chain|run#31|AssertionError|assert _HDR in hdrs, (
Issue-Key: elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-forged-clientcert-header-stripped|run#30|AssertionError|assert control is not None and len(control) == 1, (
Issue-Key: elbv2.listener-ssl-policy-bounds-the-negotiated-tls-version-and-cipher|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: elbv2.routing-http2-attribute-decides-the-alpn-protocol-the-alb-selects|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: elbv2.forward-weighted-and-sticky-target-groups-data-plane|_http#26|Timeout|except urllib.error.HTTPError as exc:
Issue-Key: elbv2.instance-target-serves-http-and-unhealthy-stops-routing|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: elbv2.zero-weight-target-group-is-never-selected|_http#29|Timeout|except urllib.error.HTTPError as exc:
Issue-Key: elbv2.xff-header-processing-mode-changes-what-the-target-receives|_http#21|Timeout|except urllib.error.HTTPError as exc:
Issue-Key: elbv2.alb-generated-responses-excluded-from-requestcount-and-target-codes|_poll_until#41|Timeout|time.sleep(interval_s)
Issue-Key: elbv2.deploy-elbv2-1-rule-update-tg-replace-deletion-protection|_wait_stack#40|Timeout|return status
Issue-Key: elbv2.redirect-action-placeholder-substitution|run#24|AssertionError|assert parsed.hostname == dns.lower(), f"#{{host}} not substituted with the request host: {location!r}"
Issue-Key: elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected|run#27|AssertionError|assert codes == ["AlreadyExists"], f"RuleA update failure codes {codes!r} from reasons {reasons!r}"
Issue-Commit: 59f14ace25ed5753b46a36483d50334ce01f6026

# elbv2: 7 net-new causes — LoadBalancer.DNSName is published as "<host>:<port>", the ALB answers 502 for a registered target, mTLS passthrough headers are missing, and TLS/data-plane checks never converge

> _Note: this issue was identified against the code base at commit `59f14ace25ed5753b46a36483d50334ce01f6026`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
Seven net-new root causes under the suite heading `elbv2` (Cause 1 of 7 .. Cause 7 of 7). All seven must be closed before this issue is submitted; each cause below carries its own failing check ids.

Related open filings that do NOT carry these replay keys, so they do not cover these failures: 20261109T153103Z-elbv2-4-net-new-causes-the-published-dnsname.md (its Cause 1 was the DNSName being an elb.amazonaws.com host; the host is now the emulator's own, so Cause 1 below is the NEXT defect in that value — the embedded port) and 20261104T094656Z-elbv2-seven-target-health-routing-and-data-plane-causes-still-open.md (its Causes 4 and 9 are adjacent to Causes 2-5 below, but the suite marked none of the ids below OWNED by it, and the observed failures have moved: the plane now answers 502 and terminates mTLS instead of never answering).

Cause 1 of 7: LoadBalancer.DNSName is published as a host WITH a port appended, so any client that joins it with a listener port builds an unusable address.
AWS: Smithy (elasticloadbalancingv2) CreateLoadBalancer -> LoadBalancers[].DNSName, documented "The public DNS name of the load balancer"; DescribeLoadBalancers repeats the same member. A DNS name is a bare hostname — it never contains a ':' or a port, because the port is chosen by the listener the client dials. A client therefore legitimately forms `http://<DNSName>:<ListenerPort>/` or resolves `<DNSName>` on its own.
Emulator: DNSName is published in the form `<host>:<port>` (replay shows a published value that, joined with listener port 80, yields `http://127.0.0.1:45601:80`; the host and the digits are per-run values, the defect is the ':<port>' in a DNS name). The harness refuses the address before connecting: `simparity.target.EndpointRefused: endpoint 'http://<host>:<port>:80' is not a http/https/ws/wss URL with a host`.
Why: a client that does the one thing a load balancer exists for — dial the name the API handed it on the listener port — produces a malformed authority and never reaches the listener. Nothing about weighted forwarding, redirects, or any listener action can be exercised through the published name.
Fix: the ELBv2 load-balancer state/serializer path that mints and reports DNSName (CreateLoadBalancer and DescribeLoadBalancers) must publish a bare reachable host with no port, and the listener socket path must accept the listener's port on that host.
Traces: elbv2.e2e-blue-green-weighted-target-group-shift, elbv2.e2e-http-to-https-redirect-traffic-flow.

Cause 2 of 7: a forward action to a registered target answers HTTP 502 forever instead of proxying the target's response.
AWS: recorded behavior only. An ALB whose target group has a registered target that answers its health check forwards the client request to that target and returns the target's status and body; HTTP 502 is reserved for the case where the target's response could not be parsed or the connection to the target failed, and HTTP 504 for a target that did not answer within the listener's idle timeout. During deregistration_delay draining the ALB keeps serving in-flight requests from the draining target.
Emulator: every probe of the listener returns `(502, '')` — an empty body — for the whole 300 s poll, for a target that is registered and reported healthy. Replay: "load balancer serving 'TARGET-A' at http://<host>:<port>/ (observed: []) not met within 300s. Last result: (502, '')"; "http://<host>:<port>/x answering 200 (observed: []) not met within 300s. Last result: 502"; and the idle-timeout check never gets its baseline 200 before it can observe a 504.
Why: a client cannot distinguish "my target is broken" from "the load balancer never forwarded": every forward-path behavior (draining, slow start, idle-timeout 504 vs 502) reads as a hard 502, and a 502 with an empty body is not a response AWS ever produces for a healthy target.
Fix: the ELBv2 data-plane forward path (the serving seam that resolves a rule's forward action to a registered target and proxies the request) must produce the target's response, and must only answer 502/504 for the modeled failure conditions.
Traces: elbv2.deregistration-delay-serves-in-flight-requests-while-draining, elbv2.elbv2shape-deep-elbv2-3-slow-start-ramps-late-target-but-not-one-call-batch, elbv2.target-response-exceeding-idle-timeout-yields-504.

Cause 3 of 7: an mTLS (passthrough) HTTPS listener forwards the request without any x-amzn-mtls-* header.
AWS: recorded behavior only (the headers are not in the Smithy model; the control-plane side is MutualAuthentication.Mode='passthrough' on CreateListener/ModifyListener). With mutual authentication in passthrough mode an ALB that received a client certificate adds `x-amzn-mtls-clientcert` (the URL-encoded PEM chain the client presented) plus the per-leaf `x-amzn-mtls-clientcert-*` headers to the request it forwards, and it STRIPS any such header the client itself sent, so the target can never be fooled by a forged one.
Emulator: the target receives NO x-amzn-* header at all — replay: "the target received no 'x-amzn-mtls-clientcert' header although the client presented a certificate; x-amzn* headers it did get: []", and in the forgery check the control arm reports "a real client certificate produced x-amzn-mtls-clientcert values None; expected one". The TLS handshake itself completed and the request reached the target, so this is the header-injection step, not termination.
Why: a target that authorizes callers from the presented certificate sees an unauthenticated request; and because nothing is injected, the emulator also cannot demonstrate that a client-supplied forgery is stripped — a client would conclude mTLS passthrough is unusable or, worse, trust a forged header.
Fix: the ELBv2 HTTPS/mTLS serving path — the step that builds the forwarded request's headers for a listener with MutualAuthentication passthrough — must add the x-amzn-mtls-clientcert family from the presented chain and remove any inbound copy first.
Traces: elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-clientcert-chain, elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-forged-clientcert-header-stripped.

Cause 4 of 7: an HTTPS listener's negotiated TLS parameters never converge — neither the SSL policy bound nor the ALPN selection can be observed.
AWS: recorded behavior only for the handshake; the control plane is Smithy Listener.SslPolicy and the load-balancer attribute routing.http2.enabled. AWS bounds the negotiated protocol version and cipher by the listener's SslPolicy (a handshake offering only an excluded version/cipher fails, an included one succeeds), and with routing.http2.enabled=true a client offering ALPN ['h2','http/1.1'] negotiates 'h2'.
Emulator: both checks exhaust their budget with no handshake outcome to grade ("Check timed out after 60.0s"), i.e. the HTTPS listener never completes a handshake a client can measure.
Why: a client cannot verify that a security policy is enforced, so an emulator-based test of a TLS hardening change always passes vacuously; HTTP/2 clients silently fall back.
Fix: the ELBv2 HTTPS listener termination seam — it must complete a handshake constrained by the listener's SslPolicy and select ALPN from the load balancer's routing.http2.enabled attribute.
Traces: elbv2.listener-ssl-policy-bounds-the-negotiated-tls-version-and-cipher, elbv2.routing-http2-attribute-decides-the-alpn-protocol-the-alb-selects.

Cause 5 of 7: the HTTP data plane never reaches a state a routing/attribute check can grade — these checks exhaust their whole budget.
AWS: recorded behavior only. A configured ALB converges within seconds: a weighted/sticky forward pins or spreads clients as configured, a zero-weight target group is never selected, an instance target that goes unhealthy stops receiving requests, routing.http.xff_header_processing.mode changes what the target receives, an ALB-generated fixed response is excluded from RequestCount and from HTTPCode_Target_*, and a CloudFormation rule/target-group update keeps serving through the change.
Emulator: every one of these checks ends with "Check timed out after 60.0s" (600.0s for the metrics one) without ever observing the configured behavior, so the ALB never converges on a servable configuration.
Why: none of the routing and attribute semantics a client builds on can be observed at all; a test suite run against the emulator neither passes nor fails these behaviors, it hangs.
Fix: the ELBv2 data-plane serving seam and the load-balancer/target-group attribute plumbing it reads (weights, stickiness, xff_header_processing, health-gated target selection), plus the metric emission path for ALB-generated responses.
Traces: elbv2.forward-weighted-and-sticky-target-groups-data-plane, elbv2.instance-target-serves-http-and-unhealthy-stops-routing, elbv2.zero-weight-target-group-is-never-selected, elbv2.xff-header-processing-mode-changes-what-the-target-receives, elbv2.alb-generated-responses-excluded-from-requestcount-and-target-codes, elbv2.deploy-elbv2-1-rule-update-tg-replace-deletion-protection.

Cause 6 of 7: a redirect action's #{host} placeholder is not substituted with the request's host.
AWS: recorded behavior only for the substitution (the control plane is Smithy RedirectActionConfig {Protocol, Port, Host, Path, Query, StatusCode}, whose members accept the reserved keywords #{protocol}, #{host}, #{port}, #{path}, #{query}). AWS replaces #{host} in the Location header with the host of the request it is answering — i.e. the load balancer's own published DNS name when the client dialed it — and #{path}/#{query} with the request's path and query.
Emulator: the Location it returns carries a host that is not the load balancer's published DNS name — replay: "#{host} not substituted with the request host: 'https://<host>:<port>/movedredir/deep/thing?orig=redir/deep/thing&keep=a=1&b=2'" (the host, port and path are the run's values; the requirement is that the Location's host equal the host of the request). The path and query substitutions did land, so only the host keyword is wrong. This is very likely a consequence of Cause 1: the published DNSName carries a ':<port>' that cannot appear in a URL host.
Why: a client following the redirect is sent to a different origin than AWS would send it to, which breaks HTTP->HTTPS redirect chains and any host-preserving redirect.
Fix: the ELBv2 redirect-action serializer in the data plane — substitute #{host} from the request's Host header (and keep it consistent with whatever CreateLoadBalancer publishes as DNSName).
Traces: elbv2.redirect-action-placeholder-substitution.

Cause 7 of 7: a CloudFormation update that collides two AWS::ElasticLoadBalancingV2::ListenerRule priorities is refused with the wrong code and message.
AWS: Smithy (elasticloadbalancingv2) SetRulePriorities declares PriorityInUseException (awsQuery code `PriorityInUse`, HTTP 400) and RuleNotFoundException (`RuleNotFound`, HTTP 400); CreateRule declares PriorityInUseException as well. A CloudFormation update whose new rule set gives two rules the same priority fails the resource, and the stack-event ResourceStatusReason the suite grades carries HandlerErrorCode `AlreadyExists` — the handler's classification of a priority that is already taken.
Emulator: the failure is reported as HandlerErrorCode `ResourceConflict` with the message `Resource handler returned message: "One or more priorities not found"` (the RequestToken is a per-run value). Both parts are wrong: the code is ResourceConflict where AWS reports AlreadyExists, and the fixed text is the RuleNotFound wording ("One or more priorities not found") where the condition is a priority already in use — AWS words that as `Priority '<priority>' is currently in use`, with the priority taken from the request.
Why: a client that branches on HandlerErrorCode (CDK/CFN rollback tooling and tests) treats an already-taken priority as a conflicting concurrent change rather than a duplicate it must renumber, and the message points the operator at a missing rule that is not missing.
Fix: the CloudFormation AWS::ElasticLoadBalancingV2::ListenerRule resource handler's priority-collision path, together with the ELBv2 CreateRule/SetRulePriorities validation that classifies the condition — a taken priority must raise PriorityInUse with the `Priority '<priority>' is currently in use` wording and surface as HandlerErrorCode AlreadyExists.
Traces: elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator: 326/613 passed (53.2%); service heading `elbv2` 202/303 (67%), 101 failed. Most of those are OWNED by open issues or ALREADY FILED against unimplemented stubs; the 17 ids in trace_ids are the survivors clustered here.

Cause 1 — [error] elbv2.e2e-blue-green-weighted-target-group-shift and [error] elbv2.e2e-http-to-https-redirect-traffic-flow:
  File "context.py", line 613, in _published_target -> validate_endpoint(...)
  File "target.py", line 154, in validate_endpoint
  simparity.target.EndpointRefused: endpoint 'http://127.0.0.1:45601:80' is not a http/https/ws/wss URL with a host
  (second check: 'http://127.0.0.1:40665:80')
  The check builds f"http://{host}:{port}" from the published DNSName and the listener port.
  Expected: DNSName is a bare host, so the joined URL has exactly one ':<port>'. Actual: DNSName already ends in ':<port>'.

Cause 2 — [mismatch] elbv2.deregistration-delay-serves-in-flight-requests-while-draining:
  AssertionError: load balancer serving 'TARGET-A' at http://127.0.0.1:35403/ (observed: []) not met within 300s. Last result: (502, '')
  [mismatch] elbv2.elbv2shape-deep-elbv2-3-slow-start-ramps-late-target-but-not-one-call-batch:
  AssertionError: http://127.0.0.1:40553/x answering 200 (observed: []) not met within 300s. Last result: 502
  [mismatch] elbv2.target-response-exceeding-idle-timeout-yields-504:
  AssertionError: load balancer serving 'SIM2REAL-TARGET-BODY' at http://127.0.0.1:33457/?s=0 (observed: []) not met within 300s. Last result: (502, '')
  Expected: HTTP 200 with the target's body. Actual: HTTP 502 with an empty body, for the entire 300 s poll.

Cause 3 — [mismatch] elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-clientcert-chain:
  AssertionError: the target received no 'x-amzn-mtls-clientcert' header although the client presented a certificate; x-amzn* headers it did get: []
  [mismatch] elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-forged-clientcert-header-stripped:
  AssertionError: control: a real client certificate produced x-amzn-mtls-clientcert values None; expected one
  Expected: exactly one x-amzn-mtls-clientcert value, derived from the presented chain. Actual: none.

Cause 4 — [mismatch] elbv2.listener-ssl-policy-bounds-the-negotiated-tls-version-and-cipher: Error: Check timed out after 60.0s
  [mismatch] elbv2.routing-http2-attribute-decides-the-alpn-protocol-the-alb-selects: Error: Check timed out after 60.0s

Cause 5 — Error: Check timed out after 60.0s for elbv2.forward-weighted-and-sticky-target-groups-data-plane, elbv2.instance-target-serves-http-and-unhealthy-stops-routing, elbv2.zero-weight-target-group-is-never-selected, elbv2.xff-header-processing-mode-changes-what-the-target-receives, elbv2.deploy-elbv2-1-rule-update-tg-replace-deletion-protection; Error: Check timed out after 600.0s for elbv2.alb-generated-responses-excluded-from-requestcount-and-target-codes.

Cause 6 — [mismatch] elbv2.redirect-action-placeholder-substitution:
  AssertionError: #{host} not substituted with the request host: 'https://127.0.0.1:8443/movedredir/deep/thing?orig=redir/deep/thing&keep=a=1&b=2'
  Expected: the Location header's host equals the host of the request (the load balancer's published DNS name). Actual: a different host.

Cause 7 — [mismatch] elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected:
  AssertionError: RuleA update failure codes ['ResourceConflict'] from reasons ['Resource handler returned message: "One or more priorities not found" (RequestToken: d58d26b1-8bfe-20ad-ae98-2a4680e88999, HandlerErrorCode: ResourceConflict)']
  Expected HandlerErrorCode: AlreadyExists, for a priority that is already in use. Actual: ResourceConflict with the RuleNotFound wording.

Smithy consulted (model services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/smithy-model/com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10.json): CreateLoadBalancer -> LoadBalancers[]; LoadBalancer$DNSName doc "The public DNS name of the load balancer"; SetRulePriorities errors PriorityInUseException (awsQuery `PriorityInUse`, 400), RuleNotFoundException (`RuleNotFound`, 400), OperationNotPermittedException; ModifyRule errors incl. RuleNotFoundException/TargetGroupNotFoundException.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; every id in trace_ids reproduces.
2. Cause 1: elbv2.create_load_balancer(Name=..., Subnets=[two subnets], Type='application'), read LoadBalancers[0].DNSName, and build f"http://{DNSName}:80/". On AWS that is a valid URL with one port; here DNSName already carries ':<port>', so the URL has two.
3. Cause 2: create a target group, register a target that answers HTTP 200 with a known body, create an HTTP:80 listener forwarding to it, wait for health, then GET the listener. Expect 200 and the target's body; the emulator returns 502 with an empty body indefinitely. Repeat with deregistration_delay set and a deregistration in flight, and with slow_start.duration_seconds set.
4. Cause 3: create an HTTPS listener with MutualAuthentication Mode='passthrough' and a trust store, connect with a client certificate, and have the target echo its request headers. Expect one x-amzn-mtls-clientcert header (plus the x-amzn-mtls-clientcert-* family); the emulator forwards none. Repeat sending a forged x-amzn-mtls-clientcert from the client and expect it stripped and replaced.
5. Cause 4: create an HTTPS listener with a restrictive SslPolicy and handshake with an excluded and an included TLS version/cipher; then set routing.http2.enabled=true and handshake offering ALPN ['h2','http/1.1']. Expect the policy to bound the handshake and 'h2' to be selected; both checks time out instead.
6. Cause 5: for each listed check, configure the ALB as it does (weighted/sticky forward, a zero-weight target group, an unhealthy instance target, routing.http.xff_header_processing.mode, a fixed-response action with RequestCount/HTTPCode_Target_* reads, a CFN rule/target-group update) and send the client requests. Expect the configured behavior within the budget; the checks exhaust it.
7. Cause 6: create an HTTP listener rule with a redirect action using Host='#{host}', Path='/moved#{path}', Query='orig=#{query}', and GET it. Expect the Location host to equal the request's host; the emulator emits another host.
8. Cause 7: deploy a stack with two AWS::ElasticLoadBalancingV2::ListenerRule resources, then update it so both carry the same Priority. Expect the resource failure to carry HandlerErrorCode AlreadyExists and the `Priority '<priority>' is currently in use` wording; the emulator reports ResourceConflict / "One or more priorities not found".
9. Re-replay the ids in trace_ids to confirm the fix.
